//! The `truncatechars` and `truncatewords` filters and their HTML variants.

use std::borrow::Cow;

use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;

use super::text::RAW_TEXT_ELEMENTS;
use super::{ResolveFilter, Text, integer_argument, string_filter_result, string_filter_text};
use crate::filters::{
    TruncateCharsFilter, TruncateCharsHtmlFilter, TruncateWordsFilter, TruncateWordsHtmlFilter,
};
use crate::render::ResolveResult;
use crate::render::types::{Content, Context, append_escaped};
use dtl_lexer::types::TemplateString;

static PGETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static COMBINING: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static TRUNCATOR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static NORMALIZE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
/// `unicodedata.combining(character) != 0` for characters already asked.
/// Python's Unicode database doesn't change while it runs.
static COMBINING_CHARACTERS: std::sync::Mutex<std::collections::BTreeMap<char, bool>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Django's translated truncation text, `%(truncated_text)s…` in English.
#[derive(Clone, Debug)]
pub struct TruncationText {
    /// The translation itself, which Django's `Truncator` accepts as `truncate`.
    text: String,
    /// The translation split around its placeholder. `None` when it is not a
    /// plain `%(truncated_text)s` template, leaving its %-formatting to Django.
    split: Option<SplitTruncationText>,
}

#[derive(Clone, Debug)]
struct SplitTruncationText {
    before: String,
    after: String,
    /// The number of characters Django counts: combining characters are free.
    width: usize,
}

impl TruncationText {
    fn new(py: Python<'_>) -> PyResult<Self> {
        let text: String = PGETTEXT
            .import(py, "django.utils.translation", "pgettext")?
            .call1((
                "String to return when truncating text",
                "%(truncated_text)s…",
            ))?
            .extract()?;
        let split = match text.split_once("%(truncated_text)s") {
            Some((before, after)) if text.matches('%').count() == 1 => {
                let combining = COMBINING.import(py, "unicodedata", "combining")?;
                let mut width = 0;
                for character in before.chars().chain(after.chars()) {
                    // Ask Python so the result follows its Unicode database.
                    if character.is_ascii() || !combining.call1((character,))?.is_truthy()? {
                        width += 1;
                    }
                }
                Some(SplitTruncationText {
                    before: before.to_string(),
                    after: after.to_string(),
                    width,
                })
            }
            _ => None,
        };
        Ok(Self { text, split })
    }
}

/// `Truncator(text).chars(length)`. `None` defers to Django's filter.
fn truncate_chars(
    text: &str,
    length: i64,
    py: Python<'_>,
    context: &mut Context,
) -> PyResult<Option<String>> {
    let Ok(length @ 1..) = usize::try_from(length) else {
        return Ok(Some(String::new()));
    };
    // ASCII characters each count once and NFC normalization leaves them alone.
    if text.is_ascii() && text.len() <= length {
        return Ok(Some(text.to_string()));
    }
    let Some(truncation) = context
        .caches()
        .truncation_text
        .get_or_try_init(py, |_| TruncationText::new(py))?
    else {
        return Ok(None);
    };
    let Some(split) = &truncation.split else {
        return Ok(None);
    };
    if !text.is_ascii() {
        return truncate_unicode_chars(text, length, split, py).map(Some);
    }
    let end = length.saturating_sub(split.width);
    Ok(Some(format!(
        "{}{}{}",
        split.before,
        &text[..end],
        split.after
    )))
}

/// Whether Python's `unicodedata.combining` gives `character` a nonzero
/// class. Nothing below U+0300 combines.
fn is_combining(character: char, py: Python<'_>) -> PyResult<bool> {
    use pyo3::sync::MutexExt;
    if character < '\u{300}' {
        return Ok(false);
    }
    let lock = || {
        COMBINING_CHARACTERS
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned")
    };
    let cached = lock().get(&character).copied();
    if let Some(combining) = cached {
        return Ok(combining);
    }
    let combining = COMBINING
        .import(py, "unicodedata", "combining")?
        .call1((character,))?
        .is_truthy()?;
    lock().insert(character, combining);
    Ok(combining)
}

/// `unicodedata.normalize("NFC", text)`. Nothing below U+0300 decomposes
/// or composes, so that text is borrowed.
fn nfc<'a>(text: &'a str, py: Python<'_>) -> PyResult<Cow<'a, str>> {
    if text.chars().all(|character| character < '\u{300}') {
        return Ok(Cow::Borrowed(text));
    }
    Ok(Cow::Owned(
        NORMALIZE
            .import(py, "unicodedata", "normalize")?
            .call1(("NFC", text))?
            .extract()?,
    ))
}

/// `Truncator._text_chars` for non-ASCII text, after Django's NFC
/// normalization. Combining characters don't count towards the length.
/// Both follow Python's Unicode database.
fn truncate_unicode_chars(
    text: &str,
    length: usize,
    split: &SplitTruncationText,
    py: Python<'_>,
) -> PyResult<String> {
    let text = nfc(text, py)?;
    let truncate_length = length.saturating_sub(split.width);
    let mut count = 0;
    let mut end = None;
    for (index, character) in text.char_indices() {
        if is_combining(character, py)? {
            continue;
        }
        count += 1;
        if end.is_none() && count > truncate_length {
            end = Some(index);
        }
        if count > length {
            let kept = &text[..end.unwrap_or(0)];
            return Ok(format!("{}{kept}{}", split.before, split.after));
        }
    }
    Ok(text.into_owned())
}

/// `Truncator(text).chars(length, html=True)`. `None` defers to Django's
/// filter.
fn truncate_chars_html(
    text: &str,
    length: i64,
    py: Python<'_>,
    context: &mut Context,
) -> PyResult<Option<String>> {
    let Ok(length @ 1..) = usize::try_from(length) else {
        return Ok(Some(String::new()));
    };
    let Some(truncation) = context
        .caches()
        .truncation_text
        .get_or_try_init(py, |_| TruncationText::new(py))?
    else {
        return Ok(None);
    };
    // Django's parser escapes the text it keeps, so ASCII text with nothing
    // to escape comes back unchanged when it fits. It fits at exactly
    // `length` characters, or within `length` less the truncation text.
    if let Some(split) = &truncation.split
        && (text.len() == length || text.len() <= length.saturating_sub(split.width))
        && text
            .bytes()
            .all(|byte| byte.is_ascii() && !matches!(byte, b'<' | b'>' | b'&' | b'"' | b'\''))
    {
        return Ok(Some(text.to_string()));
    }
    if let Some(split) = &truncation.split {
        let text = nfc(text, py)?;
        let mode = HtmlTruncation::Chars {
            length,
            processed: 0,
            remaining: length.saturating_sub(split.width),
            total: text.chars().count(),
            split,
        };
        if let Some(truncated) = truncate_html(&text, mode) {
            return Ok(Some(truncated));
        }
    }
    // Passing the cached translation saves Django's `pgettext` calls: one
    // to size the text and one for each truncation.
    // `chars(num, truncate, html)` has this order in every supported version.
    let truncated = TRUNCATOR
        .import(py, "django.utils.text", "Truncator")?
        .call1((text,))?
        .call_method1("chars", (length, &truncation.text, true))?
        .extract()?;
    Ok(Some(truncated))
}

/// `django.utils.html.VOID_ELEMENTS`.
const VOID_ELEMENTS: [&str; 16] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr", "frame", "spacer",
];

/// A piece of HTML as Python's `HTMLParser` reports it.
enum HtmlToken<'a> {
    Data(&'a str),
    /// `raw` is `get_starttag_text()`; `name` is lowercased.
    StartTag {
        raw: &'a str,
        name: String,
        self_closing: bool,
    },
    EndTag(String),
}

/// The length of a strictly well-formed start tag at the start of `text`:
/// an ASCII name, then attributes with ASCII names and quoted or plain
/// unquoted values, then `>` or ` />`. `None` for anything else, whose
/// handling has changed between versions of Python's `HTMLParser`.
fn strict_start_tag(text: &str) -> Option<(usize, String, bool)> {
    let bytes = text.as_bytes();
    let is_space = |byte: u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c');
    let mut index = 1;
    let name_length = bytes[index..]
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'-')
        .count();
    let name = text.get(index..index + name_length)?;
    if !name.starts_with(|c: char| c.is_ascii_alphabetic())
        || RAW_TEXT_ELEMENTS
            .iter()
            .any(|element| element.eq_ignore_ascii_case(name))
    {
        return None;
    }
    index += name_length;
    loop {
        let spaces = bytes[index..]
            .iter()
            .take_while(|byte| is_space(**byte))
            .count();
        index += spaces;
        match bytes.get(index)? {
            b'>' => return Some((index + 1, name.to_ascii_lowercase(), false)),
            b'/' if bytes.get(index + 1) == Some(&b'>')
                && (spaces > 0 || index == 1 + name_length) =>
            {
                return Some((index + 2, name.to_ascii_lowercase(), true));
            }
            byte if spaces > 0
                && (byte.is_ascii_alphabetic() || *byte == b'_' || *byte == b':') =>
            {
                index += bytes[index..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_alphanumeric() || b"-_:.".contains(byte))
                    .count();
                if bytes.get(index) == Some(&b'=') {
                    index += 1;
                    match bytes.get(index)? {
                        quote @ (b'"' | b'\'') => {
                            let length =
                                bytes[index + 1..].iter().position(|byte| byte == quote)?;
                            let value = &bytes[index + 1..index + 1 + length];
                            if value.iter().any(|byte| matches!(byte, b'<' | b'>' | b'&')) {
                                return None;
                            }
                            index += length + 2;
                        }
                        _ => {
                            let length = bytes[index..]
                                .iter()
                                .take_while(|byte| {
                                    !is_space(**byte) && !b"\"'=<>`/&".contains(byte)
                                })
                                .count();
                            if length == 0 {
                                return None;
                            }
                            index += length;
                        }
                    }
                }
            }
            _ => return None,
        }
    }
}

/// The length of an end tag `</name>` at the start of `text`, with optional
/// whitespace before `>`.
fn strict_end_tag(text: &str) -> Option<(usize, String)> {
    let bytes = text.as_bytes();
    let name_length = bytes[2..]
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'-')
        .count();
    let name = text.get(2..2 + name_length)?;
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut index = 2 + name_length;
    index += bytes[index..]
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c'))
        .count();
    (bytes.get(index) == Some(&b'>')).then(|| (index + 1, name.to_ascii_lowercase()))
}

/// `text` as `HTMLParser` tokenizes it, or `None` for text with entities,
/// comments, a bare `<` or tags that aren't strictly well formed.
fn html_tokens(text: &str) -> Option<Vec<HtmlToken<'_>>> {
    if text.contains('&') {
        return None;
    }
    let mut tokens = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        if start > 0 {
            tokens.push(HtmlToken::Data(&rest[..start]));
        }
        rest = &rest[start..];
        if rest.starts_with("</") {
            let (length, name) = strict_end_tag(rest)?;
            tokens.push(HtmlToken::EndTag(name));
            rest = &rest[length..];
        } else {
            let (length, name, self_closing) = strict_start_tag(rest)?;
            tokens.push(HtmlToken::StartTag {
                raw: &rest[..length],
                name,
                self_closing,
            });
            rest = &rest[length..];
        }
    }
    if !rest.is_empty() {
        tokens.push(HtmlToken::Data(rest));
    }
    Some(tokens)
}

/// What Django's `TruncateHTMLParser` subclasses count.
enum HtmlTruncation<'a> {
    /// `TruncateCharsHTMLParser`: `length` characters including the
    /// truncation text, of which `remaining` are left for data; `total` is
    /// the character count of the whole text.
    Chars {
        length: usize,
        processed: usize,
        remaining: usize,
        total: usize,
        split: &'a SplitTruncationText,
    },
    /// `TruncateWordsHTMLParser` with `truncate=" …"`.
    Words { remaining: usize },
}

/// Django's `escape()`, counting the characters written.
fn push_escaped_counted(text: &str, output: &mut String, written: &mut usize) {
    let start = output.len();
    append_escaped(text, output);
    *written += output[start..].chars().count();
}

/// The words `TruncateWordsHTMLParser` splits data into with
/// `re.split(r"(?<=\S)\s+(?=\S)", data)`: whitespace between two
/// non-whitespace characters separates them, and leading or trailing
/// whitespace stays attached.
fn html_words(data: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = 0;
    let mut previous: Option<char> = None;
    let mut run_start = None;
    for (index, character) in data.char_indices() {
        if is_python_whitespace(character) {
            if run_start.is_none() && previous.is_some_and(|c| !is_python_whitespace(c)) {
                run_start = Some(index);
            }
        } else if let Some(run) = run_start.take() {
            words.push(&data[start..run]);
            start = index;
        }
        previous = Some(character);
    }
    words.push(&data[start..]);
    words
}

/// Django's `TruncateHTMLParser.feed` on `tokens`, or `None` when the text
/// isn't tokenized natively.
fn truncate_html(text: &str, mut mode: HtmlTruncation<'_>) -> Option<String> {
    let tokens = html_tokens(text)?;
    let mut output = String::with_capacity(text.len());
    // `len(self.output)`, which the exact-fit check in `process` compares.
    let mut written = 0;
    let mut tags: Vec<String> = Vec::new();
    let is_void = |name: &str| VOID_ELEMENTS.contains(&name);
    let end_tag = |name: &str, output: &mut String, written: &mut usize, tags: &mut Vec<String>| {
        if !is_void(name) {
            output.push_str("</");
            output.push_str(name);
            output.push('>');
            *written += name.chars().count() + 3;
            if tags.last().is_some_and(|last| last == name) {
                tags.pop();
            }
        }
    };
    for token in tokens {
        match token {
            HtmlToken::StartTag {
                raw,
                name,
                self_closing,
            } => {
                output.push_str(raw);
                written += raw.chars().count();
                if !is_void(&name) {
                    tags.push(name.clone());
                }
                if self_closing {
                    end_tag(&name, &mut output, &mut written, &mut tags);
                }
            }
            HtmlToken::EndTag(name) => end_tag(&name, &mut output, &mut written, &mut tags),
            HtmlToken::Data(data) => {
                let completed = match &mut mode {
                    HtmlTruncation::Chars {
                        length,
                        processed,
                        remaining,
                        total,
                        split,
                    } => {
                        let data_length = data.chars().count();
                        *processed += data_length;
                        if *processed == *length && written + data_length == *total {
                            output.push_str(data);
                            true
                        } else if *remaining < data_length {
                            let end = data
                                .char_indices()
                                .nth(*remaining)
                                .map_or(data.len(), |(index, _)| index);
                            output.push_str(&split.before);
                            append_escaped(&data[..end], &mut output);
                            output.push_str(&split.after);
                            true
                        } else {
                            *remaining -= data_length;
                            push_escaped_counted(data, &mut output, &mut written);
                            false
                        }
                    }
                    HtmlTruncation::Words { remaining } => {
                        let words = html_words(data);
                        let kept = words[..(*remaining).min(words.len())].join(" ");
                        if *remaining < words.len() {
                            let start = output.len();
                            append_escaped(&kept, &mut output);
                            if !output[start..].ends_with(" …") {
                                output.push_str(" …");
                            }
                            true
                        } else {
                            *remaining -= words.len();
                            push_escaped_counted(&kept, &mut output, &mut written);
                            false
                        }
                    }
                };
                if completed {
                    for tag in tags.iter().rev() {
                        output.push_str("</");
                        output.push_str(tag);
                        output.push('>');
                    }
                    return Some(output);
                }
            }
        }
    }
    Some(output)
}

impl ResolveFilter for TruncateWordsHtmlFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(length) = argument.value().and_then(integer_argument)
            && let Some(text) = Text::from_variable(variable.as_ref(), py)?
        {
            let truncated = match usize::try_from(length) {
                Err(_) | Ok(0) => Some(String::new()),
                Ok(remaining) => truncate_html(text.as_str(), HtmlTruncation::Words { remaining }),
            };
            if let Some(truncated) = truncated {
                return Ok(Some(text.into_content(truncated, context)));
            }
        }
        self.django.call(variable, argument, py, context)
    }
}

impl ResolveFilter for TruncateCharsHtmlFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(length) = argument.value().and_then(integer_argument)
            && let Some(text) = Text::from_variable(variable.as_ref(), py)?
            && let Some(truncated) = truncate_chars_html(text.as_str(), length, py, context)?
        {
            return Ok(Some(text.into_content(truncated, context)));
        }
        self.django.call(variable, argument, py, context)
    }
}

impl ResolveFilter for TruncateCharsFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        let length = argument.value().and_then(integer_argument);
        if let Some(length) = length
            && let Some(text) = Text::from_variable(variable.as_ref(), py)?
            && let Some(truncated) = truncate_chars(text.as_str(), length, py, context)?
        {
            return Ok(Some(text.into_content(truncated, context)));
        }
        self.django.call(variable, argument, py, context)
    }
}

/// Whitespace as Python's `str.split()` sees it: Unicode `White_Space` plus
/// the information separators U+001C to U+001F.
pub(super) fn is_python_whitespace(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// `Truncator(text).words(length, truncate=" …")`: the first `length` words
/// joined by single spaces, then " …" when words were dropped. `truncatewords`
/// passes this fixed text, so no translation is involved.
fn truncate_words(text: &str, length: i64) -> String {
    const TRUNCATE: &str = " …";
    let Ok(length @ 1..) = usize::try_from(length) else {
        return String::new();
    };
    let mut words = text
        .split(is_python_whitespace)
        .filter(|word| !word.is_empty());
    let mut truncated = String::with_capacity(text.len());
    for word in words.by_ref().take(length) {
        if !truncated.is_empty() {
            truncated.push(' ');
        }
        truncated.push_str(word);
    }
    // Django doesn't append the truncation text when the words already end with it.
    if words.next().is_some() && !truncated.ends_with(TRUNCATE) {
        truncated.push_str(TRUNCATE);
    }
    truncated
}

impl ResolveFilter for TruncateWordsFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(length) = argument.value().and_then(integer_argument) {
            // `@stringfilter`: other values are converted with `str()`.
            let (text, safe) = string_filter_text(variable, py)?;
            let truncated = truncate_words(&text, length);
            return Ok(Some(string_filter_result(truncated, safe, context)));
        }
        self.django.call(variable, argument, py, context)
    }
}
