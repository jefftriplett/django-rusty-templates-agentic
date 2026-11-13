//! String filters: case, padding, wrapping, linebreaks, encoding, `urlize`, `stringformat` and friends.

use std::borrow::Cow;
use std::collections::HashMap;

use html_escape::encode_quoted_attribute_to_string;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use pyo3::exceptions::{PyAttributeError, PyTypeError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};

use super::number::python_float_str;
use super::python::ResolvedArgument;
use super::truncate::is_python_whitespace;
use super::{
    ResolveFilter, SAFEDATA, SAFESTRING, Text, integer_argument, is_lazy_string, string_argument,
    string_filter_result, string_filter_text,
};
use crate::error::{AnnotatePyErr, PyRenderError, RenderError};
use crate::filters::{
    AddSlashesFilter, CapfirstFilter, CenterFilter, CutFilter, IriencodeFilter, Justify,
    JustifyFilter, LinebreaksFilter, LinebreaksbrFilter, LinenumbersFilter, LowerFilter,
    Phone2numericFilter, PluralizeFilter, SlugifyFilter, StringformatFilter, StriptagsFilter,
    TitleFilter, UpperFilter, UrlencodeFilter, UrlizeFilter, UrlizetruncFilter, WordcountFilter,
    WordwrapFilter, YesnoFilter,
};
use crate::render::common::gettext;
use crate::render::types::{
    AsBorrowedContent, Content, ContentString, Context, IntoOwnedContent, append_escaped,
};
use crate::render::urlize::{UrlizeRules, handle_word};
use crate::render::{Resolve, ResolveFailures, ResolveResult};
use dtl_lexer::types::TemplateString;
use unicode_normalization::UnicodeNormalization;

/// Point Python's own `int(arg)` error at the argument.
fn annotate_argument(
    error: PyRenderError,
    py: Python<'_>,
    at: dtl_lexer::types::At,
    template: TemplateString<'_>,
) -> PyRenderError {
    match error {
        PyRenderError::PyErr(error) => error.annotate(py, at, "argument", template).into(),
        error => error,
    }
}

impl ResolveFilter for AddSlashesFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        _py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let content = match variable {
            Some(content) => {
                let content_string = content.resolve_string(context)?;
                content_string.map_content(|raw| {
                    Cow::Owned(
                        raw.replace('\\', r"\\")
                            .replace('"', "\\\"")
                            .replace('\'', r"\'"),
                    )
                })
            }
            None => "".as_content(),
        };
        Ok(Some(content))
    }
}

impl ResolveFilter for CapfirstFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        _py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let content = match variable {
            Some(content) => {
                let content_string = content.resolve_string(context)?;
                let mut chars = content_string.as_raw().chars();
                let first_char = match chars.next() {
                    Some(c) => c.to_uppercase(),
                    None => return Ok(Some("".as_content())),
                };
                let string: String = first_char.chain(chars).collect();
                content_string.map_content(|_| Cow::Owned(string))
            }
            None => "".as_content(),
        };
        Ok(Some(content))
    }
}

impl ResolveFilter for CenterFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Django resolves a filter's arguments before calling it, so a
        // missing argument raises even for a missing value.
        let argument = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?;
        let Some(content) = variable else {
            return Ok(Some("".as_content()));
        };
        let content = content.resolve_string(context)?;
        let length = content.as_raw().chars().count();
        let arg = argument.expect("missing argument in context should already have raised");

        if context.django_series(py)? >= (6, 0) {
            // Django 6.0's `f"{value:^{width}}"` returns the value for a width
            // of zero or less and puts the odd padding space on the right.
            if let Some(width) = integer_argument(&arg)
                && usize::try_from(width).map_or(true, |width| width <= MAX_JUSTIFY_WIDTH)
            {
                let Ok(width @ 1..) = usize::try_from(width) else {
                    return Ok(Some(Content::String(content)));
                };
                let padding = width.saturating_sub(length);
                let left = padding / 2;
                let mut centered = String::with_capacity(content.as_raw().len() + padding);
                centered.extend(std::iter::repeat_n(' ', left));
                centered.push_str(content.as_raw());
                centered.extend(std::iter::repeat_n(' ', padding - left));
                return Ok(Some(content.map_content(|_| Cow::Owned(centered))));
            }
            let centered = py
                .import("django.template.defaultfilters")?
                .getattr("center")?
                .call1((content.as_raw(), arg.to_py(py)))?
                .extract::<String>()?;
            return Ok(Some(content.map_content(|_| Cow::Owned(centered))));
        }

        // Django 5.2's `value.center(int(arg))`; a negative width leaves the
        // value unchanged.
        let size = match integer_argument(&arg) {
            Some(width) => usize::try_from(width).unwrap_or(0),
            None => arg
                .resolve_usize(self.argument.at)
                .map_err(|error| annotate_argument(error, py, self.argument.at, template))?,
        };

        if size <= length {
            return Ok(Some(Content::String(content)));
        }
        if size > MAX_JUSTIFY_WIDTH {
            // Python raises `MemoryError` or `OverflowError` for a width it
            // can't allocate; allocating it here would abort the process.
            let centered = PyString::new(py, content.as_raw())
                .call_method1(intern!(py, "center"), (size,))?
                .extract::<String>()?;
            return Ok(Some(content.map_content(|_| Cow::Owned(centered))));
        }
        let round_up = size % 2 == 0 && length % 2 != 0;
        let right = if round_up {
            // If the size is even and the content length is odd, we need to adjust the centering
            (size - length).div_ceil(2)
        } else {
            (size - length) / 2
        };
        let left = size - length - right;

        let mut centered = String::with_capacity(size);

        centered.push_str(&" ".repeat(left));
        centered.push_str(content.as_raw());
        centered.push_str(&" ".repeat(right));

        Ok(Some(content.map_content(|_| Cow::Owned(centered))))
    }
}

fn cut(source: Cow<'_, str>, value: &str) -> String {
    source.replace(value, "")
}

impl ResolveFilter for CutFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Django resolves a filter's arguments before calling it, so a
        // missing argument raises even for a missing value.
        let argument = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?;
        let Some(variable) = variable else {
            return Ok(Some("".as_content()));
        };

        let arg = string_argument(
            argument.expect("missing argument in context should already have raised"),
            context,
            (py, self.argument.at, template),
            |name| PyTypeError::new_err(format!("replace() argument 1 must be str, not {name}")),
        )?;

        let content_string = variable.resolve_string(context)?;
        let result = match content_string {
            ContentString::String(s) => ContentString::String(cut(s, &arg).into()),
            ContentString::HtmlSafe(s) => {
                let cut = cut(s, &arg).into();
                match arg.as_ref() {
                    ";" => ContentString::HtmlUnsafe(cut),
                    _ => ContentString::HtmlSafe(cut),
                }
            }
            ContentString::HtmlUnsafe(s) => ContentString::HtmlUnsafe(cut(s, &arg).into()),
        };

        Ok(Some(Content::String(result)))
    }
}

impl ResolveFilter for LowerFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        _py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let content = match variable {
            Some(content) => content
                .resolve_string(context)?
                .map_content(|content| Cow::Owned(content.to_lowercase())),
            None => "".as_content(),
        };
        Ok(Some(content))
    }
}

/// Convert spaces or repeated dashes to single dashes.
///
/// Remove characters that aren't ascii alphanumerics, underscores, or hyphens.
/// Convert to lowercase. Also strip leading and trailing whitespace, dashes, and underscores.
///
/// See <https://github.com/django/django/blob/stable/5.2.x/django/utils/text.py#L453>
pub fn slugify(content: Cow<str>) -> Cow<str> {
    let mut slug = String::with_capacity(content.len());
    let mut previous_dash = false;
    for c in content.as_ref().nfkd().filter(char::is_ascii) {
        if c.is_ascii_alphanumeric() || c == '_' {
            slug.push(c.to_ascii_lowercase());
            previous_dash = false;
        // Python's `\s` also matches the separators U+001C to U+001F.
        } else if !previous_dash
            && (c == '-' || c.is_ascii_whitespace() || matches!(c, '\x0b' | '\x1c'..='\x1f'))
        {
            slug.push('-');
            previous_dash = true;
        }
    }
    // Django's `.strip("-_")`.
    Cow::Owned(slug.trim_matches(['-', '_']).to_string())
}

impl ResolveFilter for SlugifyFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        _context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let content = match variable {
            Some(content) => match content {
                Content::Py(content) => {
                    let slug = slugify(Cow::Owned(content.str()?.extract::<String>()?));
                    #[allow(non_snake_case)]
                    let SafeData = SAFEDATA.import(py, "django.utils.safestring", "SafeData")?;
                    match content.is_instance(SafeData)? {
                        true => Content::String(ContentString::HtmlSafe(slug)),
                        false => Content::String(ContentString::HtmlUnsafe(slug)),
                    }
                }
                // `slugify` drops the sign and the decimal point of a number.
                Content::Int(content) => slugify(Cow::Owned(content.to_string()))
                    .into_owned()
                    .into_content(),
                Content::Float(content) => slugify(Cow::Owned(python_float_str(content)))
                    .into_owned()
                    .into_content(),
                Content::String(content) => content.map_content(slugify),
                Content::Bool(true) => "true".as_content(),
                Content::Bool(false) => "false".as_content(),
            },
            None => "".as_content(),
        };
        Ok(Some(content))
    }
}

/// Widths beyond this go to Django, which raises its own `MemoryError` or
/// `OverflowError` instead of allocating here.
const MAX_JUSTIFY_WIDTH: usize = 1 << 20;

impl ResolveFilter for JustifyFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(width) = argument.value().and_then(integer_argument)
            && usize::try_from(width).map_or(true, |width| width <= MAX_JUSTIFY_WIDTH)
        {
            // `@stringfilter`: other values are converted with `str()`.
            // `str.ljust` and `str.rjust` count code points and leave text
            // at least `width` long unchanged.
            let (content, safe) = string_filter_text(variable, py)?;
            let length = content.chars().count();
            let padding = usize::try_from(width).unwrap_or(0).saturating_sub(length);
            let mut justified = String::with_capacity(content.len() + padding);
            if self.side == Justify::Right {
                justified.extend(std::iter::repeat_n(' ', padding));
            }
            justified.push_str(&content);
            if self.side == Justify::Left {
                justified.extend(std::iter::repeat_n(' ', padding));
            }
            return Ok(Some(string_filter_result(justified, safe, context)));
        }
        self.django.call(variable, argument, py, context)
    }
}

/// Whether `pluralize` picks the singular suffix, or `None` to leave the
/// value to Django: `float(value) == 1` for numbers, `len(value) == 1` for
/// sized values. Strings stay with Django, whose `float()` accepts forms
/// such as `" 1 "` and `"1_0"`.
fn pluralize_singular(value: &Content<'_, '_>) -> Option<bool> {
    match value {
        // `float()` of an int too big for a float raises `OverflowError`.
        Content::Int(n) => n.to_i64().map(|n| n == 1),
        #[expect(clippy::float_cmp, reason = "Django compares `float(value) == 1`")]
        Content::Float(n) => Some(*n == 1.0),
        Content::Bool(b) => Some(*b),
        Content::String(_) => None,
        Content::Py(object) => {
            if object.is_exact_instance_of::<PyInt>() {
                object.extract::<i64>().ok().map(|n| n == 1)
            } else if object.is_exact_instance_of::<PyBool>() {
                object.extract::<bool>().ok()
            } else if object.is_exact_instance_of::<PyFloat>() {
                #[expect(clippy::float_cmp, reason = "Django compares `float(value) == 1`")]
                object.extract::<f64>().ok().map(|n| n == 1.0)
            } else if object.is_exact_instance_of::<PyList>()
                || object.is_exact_instance_of::<PyTuple>()
                || object.is_exact_instance_of::<PyDict>()
            {
                object.len().ok().map(|len| len == 1)
            } else {
                None
            }
        }
    }
}

impl ResolveFilter for PluralizeFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        let suffixes = match &argument {
            ResolvedArgument::Absent => Some(Cow::Borrowed("s")),
            ResolvedArgument::Present(Some(Content::String(text))) => {
                Some(Cow::Borrowed(text.as_raw().as_ref()))
            }
            ResolvedArgument::Present(Some(Content::Py(object))) => object
                .cast::<PyString>()
                .ok()
                .and_then(|text| text.to_str().ok().map(|text| Cow::Owned(text.to_string()))),
            ResolvedArgument::Present(_) => None,
        };
        if let Some(suffixes) = suffixes {
            let (singular, plural) = suffixes.split_once(',').unwrap_or(("", &suffixes));
            // More than two suffixes give "" whatever the value.
            if plural.contains(',') {
                return Ok(Some("".as_content()));
            }
            let singular_value = match &variable {
                // Django's filter gets "", and `float("")` fails.
                None => Some(None),
                Some(value) => pluralize_singular(value).map(Some),
            };
            if let Some(singular_value) = singular_value {
                let suffix = match singular_value {
                    None => "",
                    Some(true) => singular,
                    Some(false) => plural,
                };
                // The suffix is a plain `str`, so it is escaped.
                return Ok(Some(Content::String(ContentString::HtmlUnsafe(
                    Cow::Owned(suffix.to_string()),
                ))));
            }
        }
        self.django.call(variable, argument, py, context)
    }
}

/// Python's `urllib.parse.quote(text, safe)`: UTF-8 bytes outside
/// `A-Za-z0-9_.-~` and `safe` become `%XX`.
fn quote(text: &str, safe: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"_.-~".contains(&byte) || safe.contains(&byte) {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0xF)]));
        }
    }
    output
}

impl ResolveFilter for Phone2numericFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Python's `str.lower()` differs from Rust's for some non-ASCII
        // characters, so only ASCII text is converted here.
        if let Some(text) = Text::from_variable(variable.as_ref(), py)?
            && text.as_str().is_ascii()
        {
            let numeric = text
                .as_str()
                .bytes()
                .map(|byte| match byte.to_ascii_lowercase() {
                    b'a'..=b'c' => '2',
                    b'd'..=b'f' => '3',
                    b'g'..=b'i' => '4',
                    b'j'..=b'l' => '5',
                    b'm'..=b'o' => '6',
                    b'p'..=b's' => '7',
                    b't'..=b'v' => '8',
                    b'w'..=b'z' => '9',
                    other => char::from(other),
                })
                .collect();
            return Ok(Some(text.into_content(numeric, context)));
        }
        self.django.resolve(variable, py, template, context)
    }
}

impl ResolveFilter for UrlencodeFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        let safe = match &argument {
            // Django passes no `safe`, so `quote()` keeps its default "/".
            ResolvedArgument::Absent | ResolvedArgument::Present(None) => Some(Cow::Borrowed("/")),
            ResolvedArgument::Present(Some(Content::String(text))) => {
                Some(Cow::Borrowed(text.as_raw().as_ref()))
            }
            ResolvedArgument::Present(Some(Content::Py(object))) => object
                .cast::<PyString>()
                .ok()
                .and_then(|text| text.to_str().ok().map(|text| Cow::Owned(text.to_string()))),
            ResolvedArgument::Present(Some(_)) => None,
        };
        // `quote()` drops non-ASCII `safe` characters; leave those to Django.
        if let Some(safe) = safe.filter(|safe| safe.is_ascii()) {
            let (text, _) = string_filter_text(variable, py)?;
            let quoted = quote(&text, safe.as_bytes());
            return Ok(Some(Content::String(ContentString::HtmlUnsafe(
                Cow::Owned(quoted),
            ))));
        }
        self.django.call(variable, argument, py, context)
    }
}

impl ResolveFilter for IriencodeFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        _context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // `iri_to_uri()`'s safe characters, unchanged from 5.2 to main.
        let (text, safe) = string_filter_text(variable, py)?;
        let quoted = Cow::Owned(quote(&text, b"/#%[]=:;$&()+,!?*@'~"));
        Ok(Some(Content::String(if safe {
            ContentString::HtmlSafe(quoted)
        } else {
            ContentString::HtmlUnsafe(quoted)
        })))
    }
}

/// Elements whose content Python's HTML parser reads as raw text.
pub(super) const RAW_TEXT_ELEMENTS: [&str; 10] = [
    "iframe",
    "noembed",
    "noframes",
    "noscript",
    "plaintext",
    "script",
    "style",
    "textarea",
    "title",
    "xmp",
];

/// The length of the simple tag at the start of `text`, which begins with
/// `<`: a start or end tag with an ASCII name and plain attributes, with no
/// `<` or `>` inside it. Returns `None` for anything else, such as comments,
/// raw text elements or a bare `<`, whose handling depends on the version of
/// Python's HTML parser.
fn simple_tag_length(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let end_tag = bytes.get(1) == Some(&b'/');
    let name_start = if end_tag { 2 } else { 1 };
    let name_length = bytes
        .get(name_start..)?
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'-')
        .count();
    let name = text.get(name_start..name_start + name_length)?;
    if !name.starts_with(|c: char| c.is_ascii_alphabetic())
        || RAW_TEXT_ELEMENTS
            .iter()
            .any(|element| element.eq_ignore_ascii_case(name))
    {
        return None;
    }
    let mut index = name_start + name_length;
    if !matches!(bytes.get(index), Some(b'>' | b'/' | b' ' | b'\t' | b'\n')) {
        return None;
    }
    let mut quote = None;
    let mut previous = 0;
    loop {
        let byte = *bytes.get(index)?;
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) if byte == b'<' || byte == b'>' => return None,
            Some(_) => {}
            None => match byte {
                b'>' => return Some(index + 1),
                b'<' => return None,
                // Quotes only open an attribute value in a start tag.
                b'"' | b'\'' if !end_tag && previous == b'=' => quote = Some(byte),
                b'"' | b'\'' | b'`' => return None,
                _ => {}
            },
        }
        previous = byte;
        index += 1;
    }
}

/// Django's `strip_tags` for text whose tags are all simple, or `None` to
/// leave the text to Django. Entities are left to Django too, because its
/// parser rewrites some of them.
fn strip_simple_tags(text: &str) -> Option<String> {
    if !text.contains('<') {
        return Some(text.to_owned());
    }
    if text.contains('&') {
        return None;
    }
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        let length = simple_tag_length(&rest[start..])?;
        rest = &rest[start + length..];
    }
    output.push_str(rest);
    Some(output)
}

impl ResolveFilter for StriptagsFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        if let Some(text) = Text::from_variable(variable.as_ref(), py)?
            && let Some(stripped) = strip_simple_tags(text.as_str())
        {
            return Ok(Some(text.into_content(stripped, context)));
        }
        self.django.resolve(variable, py, template, context)
    }
}

/// How `%` formatting pads to the width.
#[derive(Default, PartialEq, Eq)]
enum FormatAlign {
    #[default]
    Right,
    /// The `-` flag, which overrides `0`.
    Left,
    /// The `0` flag, for numbers.
    Zeros,
}

/// What `%` formatting puts before a positive number.
#[derive(Default)]
enum PositiveSign {
    #[default]
    Nothing,
    /// The ` ` flag.
    Space,
    /// The `+` flag, which overrides ` `.
    Plus,
}

/// A `%` specifier such as `05d` or `-10s`, as `stringformat` takes it.
#[derive(Default)]
struct FormatSpec {
    align: FormatAlign,
    positive: PositiveSign,
    alternate: bool,
    width: usize,
    precision: Option<usize>,
    conversion: u8,
}

impl FormatSpec {
    /// Parses one complete specifier, or returns `None` for anything else,
    /// such as mapping keys, `*` widths or trailing text.
    fn parse(spec: &str) -> Option<Self> {
        let bytes = spec.as_bytes();
        let mut result = Self::default();
        let mut index = 0;
        while let Some(&flag) = bytes.get(index) {
            match flag {
                b'-' => result.align = FormatAlign::Left,
                b'0' if result.align == FormatAlign::Right => result.align = FormatAlign::Zeros,
                b'0' => {}
                b'+' => result.positive = PositiveSign::Plus,
                b' ' if matches!(result.positive, PositiveSign::Nothing) => {
                    result.positive = PositiveSign::Space;
                }
                b' ' => {}
                b'#' => result.alternate = true,
                _ => break,
            }
            index += 1;
        }
        let digits = |index: &mut usize| -> Option<usize> {
            let start = *index;
            while bytes.get(*index).is_some_and(u8::is_ascii_digit) {
                *index += 1;
            }
            let text = spec.get(start..*index)?;
            // Keep widths small; Django's formatting handles the rest.
            if text.len() > 3 {
                return None;
            }
            Some(text.parse().unwrap_or(0))
        };
        result.width = digits(&mut index)?;
        if bytes.get(index) == Some(&b'.') {
            index += 1;
            result.precision = Some(digits(&mut index)?);
        }
        result.conversion = *bytes.get(index)?;
        (index + 1 == bytes.len()).then_some(result)
    }

    /// Pads `body`, whose sign and prefix are `sign`, to the width.
    fn pad(&self, sign: &str, body: &str, numeric: bool) -> String {
        let length = sign.chars().count() + body.chars().count();
        let fill = self.width.saturating_sub(length);
        let mut output = String::with_capacity(length + fill);
        match self.align {
            FormatAlign::Left => {
                output.push_str(sign);
                output.push_str(body);
                output.extend(std::iter::repeat_n(' ', fill));
            }
            FormatAlign::Zeros if numeric => {
                output.push_str(sign);
                output.extend(std::iter::repeat_n('0', fill));
                output.push_str(body);
            }
            FormatAlign::Zeros | FormatAlign::Right => {
                output.extend(std::iter::repeat_n(' ', fill));
                output.push_str(sign);
                output.push_str(body);
            }
        }
        output
    }

    const fn sign(&self, negative: bool) -> &'static str {
        if negative {
            return "-";
        }
        match self.positive {
            PositiveSign::Nothing => "",
            PositiveSign::Space => " ",
            PositiveSign::Plus => "+",
        }
    }

    fn format_text(&self, text: &str) -> Option<String> {
        if self.conversion != b's' {
            return None;
        }
        let text = match self.precision {
            Some(precision) => match text.char_indices().nth(precision) {
                Some((end, _)) => &text[..end],
                None => text,
            },
            None => text,
        };
        Some(self.pad("", text, false))
    }

    fn format_int(&self, value: i64) -> Option<String> {
        let magnitude = value.unsigned_abs();
        let (digits, prefix) = match self.conversion {
            b's' => return self.format_text(&value.to_string()),
            b'd' | b'i' => (magnitude.to_string(), ""),
            b'x' => (format!("{magnitude:x}"), "0x"),
            b'X' => (format!("{magnitude:X}"), "0X"),
            b'o' => (format!("{magnitude:o}"), "0o"),
            b'f' | b'F' | b'e' | b'E' => {
                // Python converts to float, which is exact up to 2**53.
                return if magnitude <= 1 << 53 {
                    #[allow(clippy::cast_precision_loss)]
                    self.format_float(value as f64)
                } else {
                    None
                };
            }
            _ => return None,
        };
        let digits = match self.precision {
            Some(_) if self.align == FormatAlign::Zeros => return None,
            Some(precision) if precision > digits.len() => {
                format!("{}{digits}", "0".repeat(precision - digits.len()))
            }
            _ => digits,
        };
        let prefix = if self.alternate && !prefix.is_empty() {
            prefix
        } else {
            ""
        };
        let sign = format!("{}{prefix}", self.sign(value < 0));
        Some(self.pad(&sign, &digits, true))
    }

    fn format_float(&self, value: f64) -> Option<String> {
        if !value.is_finite() || self.alternate {
            return None;
        }
        let precision = self.precision.unwrap_or(6);
        let magnitude = value.abs();
        let body = match self.conversion {
            b'f' | b'F' => format!("{magnitude:.precision$}"),
            b'e' | b'E' => {
                let formatted = format!("{magnitude:.precision$e}");
                let (mantissa, exponent) = formatted.split_once('e')?;
                let exponent: i32 = exponent.parse().ok()?;
                let sign = if exponent < 0 { '-' } else { '+' };
                let marker = if self.conversion == b'E' { 'E' } else { 'e' };
                format!("{mantissa}{marker}{sign}{:02}", exponent.unsigned_abs())
            }
            _ => return None,
        };
        Some(self.pad(self.sign(value.is_sign_negative()), &body, true))
    }
}

/// The argument of `stringformat` as text, when it is a string.
pub(super) fn format_spec_text<'a>(argument: &'a Content<'_, '_>) -> Option<&'a str> {
    match argument {
        Content::String(
            ContentString::String(text)
            | ContentString::HtmlSafe(text)
            | ContentString::HtmlUnsafe(text),
        ) => Some(text),
        Content::Py(object) if object.is_exact_instance_of::<PyString>() => {
            object.cast::<PyString>().ok()?.to_str().ok()
        }
        _ => None,
    }
}

impl ResolveFilter for StringformatFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(spec) = argument
            .value()
            .and_then(format_spec_text)
            .and_then(FormatSpec::parse)
        {
            let number = |output: Option<String>| {
                output.map(|output| Content::String(ContentString::String(Cow::Owned(output))))
            };
            let formatted = match &variable {
                Some(Content::Int(value)) => number(
                    i64::try_from(value)
                        .ok()
                        .and_then(|value| spec.format_int(value)),
                ),
                Some(Content::Float(value)) => number(spec.format_float(*value)),
                Some(Content::Py(value)) if value.is_exact_instance_of::<PyInt>() => number(
                    value
                        .extract::<i64>()
                        .ok()
                        .and_then(|value| spec.format_int(value)),
                ),
                Some(Content::Py(value)) if value.is_exact_instance_of::<PyFloat>() => number(
                    value
                        .extract::<f64>()
                        .ok()
                        .and_then(|value| spec.format_float(value)),
                ),
                _ => match Text::from_variable(variable.as_ref(), py)? {
                    Some(text) => spec
                        .format_text(text.as_str())
                        .map(|output| text.into_content(output, context)),
                    None => None,
                },
            };
            if let Some(formatted) = formatted {
                return Ok(Some(formatted));
            }
        }
        self.django.call(variable, argument, py, context)
    }
}

static HTML_MODULE: PyOnceLock<Py<PyModule>> = PyOnceLock::new();
static HTML_UNESCAPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Whether `word` might become a link in Django's `Urlizer.handle_word`.
/// A link needs `http`, `www.`, `@` or one of the seven gTLDs after a dot,
/// and its middle is a substring of the word. Non-ASCII words with `.`, `@`
/// or `:` are left to Python, because `re.IGNORECASE` matches some
/// non-ASCII letters.
fn may_urlize(word: &str) -> bool {
    if !word.contains(['.', '@', ':']) {
        return false;
    }
    if !word.is_ascii() {
        return true;
    }
    let lower = word.to_ascii_lowercase();
    lower.contains("http")
        || lower.contains("www.")
        || lower.contains('@')
        || [".com", ".edu", ".gov", ".int", ".mil", ".net", ".org"]
            .iter()
            .any(|tld| lower.contains(tld))
}

/// The `urlize` rules for this Django series, or `None` to leave every link
/// to Django: for a series that isn't ported, or when `django.utils.html`'s
/// `urlizer` has been replaced or customized.
fn urlize_rules(py: Python<'_>, series: (u8, u8)) -> PyResult<Option<UrlizeRules>> {
    let html = HTML_MODULE.import(py, "django.utils", "html")?;
    let urlizer = html.getattr("urlizer")?;
    if !urlizer.get_type().is(&html.getattr("Urlizer")?) {
        return Ok(None);
    }
    let stock = [
        ("trailing_punctuation_chars", ".,:;!"),
        ("mailto_template", "mailto:{local}@{domain}"),
        ("url_template", "<a href=\"{href}\"{attrs}>{url}</a>"),
    ];
    for (name, value) in stock {
        if urlizer.getattr(name)?.extract::<String>().ok().as_deref() != Some(value) {
            return Ok(None);
        }
    }
    let wrapping: Option<Vec<(String, String)>> =
        urlizer.getattr("wrapping_punctuation")?.extract().ok();
    let expected = [("(", ")"), ("[", "]")];
    if wrapping.is_none_or(|pairs| {
        pairs.len() != expected.len()
            || pairs
                .iter()
                .zip(expected)
                .any(|((open, close), (o, c))| open != o || close != c)
    }) {
        return Ok(None);
    }
    let assume_https = if series >= (6, 0) {
        py.import("django.conf")?
            .getattr("settings")?
            .getattr("URLIZE_ASSUME_HTTPS")
            .map_or(Ok(false), |value| value.is_truthy())?
    } else {
        false
    };
    Ok(UrlizeRules::for_series(series, assume_https))
}

/// Django's `urlize(text, trim_url_limit, nofollow=True, autoescape)`.
/// Words that can't become links are escaped as `handle_word` does, and
/// most links are built by `urlize::handle_word`. The rest go through
/// Django's `urlizer`, once per distinct word.
fn urlize_text(
    text: &str,
    safe: bool,
    limit: Option<i64>,
    autoescape: bool,
    series: (u8, u8),
    py: Python<'_>,
) -> PyResult<String> {
    // `word_split_re`: runs of whitespace, `<`, `>`, `"` and `'` separate words.
    let is_separator =
        |character: char| is_python_whitespace(character) || "<>\"'".contains(character);
    let mut output = String::with_capacity(text.len());
    let mut links: HashMap<&str, String> = HashMap::new();
    let mut urlizer = None;
    let mut rules: Option<Option<UrlizeRules>> = None;
    let mut rest = text;
    while !rest.is_empty() {
        let separator = rest.starts_with(is_separator);
        let end = rest
            .find(|character: char| is_separator(character) != separator)
            .unwrap_or(rest.len());
        let (piece, remaining) = rest.split_at(end);
        rest = remaining;
        if separator || !may_urlize(piece) {
            if autoescape && !safe {
                append_escaped(piece, &mut output);
            } else {
                output.push_str(piece);
            }
            continue;
        }
        if let Some(link) = links.get(piece) {
            output.push_str(link);
            continue;
        }
        let rules = match rules {
            Some(rules) => rules,
            None => *rules.insert(urlize_rules(py, series)?),
        };
        let mut unescape = |text: &str| -> PyResult<String> {
            HTML_UNESCAPE
                .import(py, "html", "unescape")?
                .call1((text,))?
                .extract()
        };
        if let Some(rules) = rules
            && let Some(link) = handle_word(piece, safe, limit, autoescape, rules, &mut unescape)?
        {
            output.push_str(&link);
            links.insert(piece, link);
            continue;
        }
        let urlizer = match &urlizer {
            Some(urlizer) => urlizer,
            None => urlizer.insert(
                HTML_MODULE
                    .import(py, "django.utils", "html")?
                    .getattr("urlizer")?,
            ),
        };
        let word = if safe {
            SAFESTRING
                .import(py, "django.utils.safestring", "SafeString")?
                .call1((piece,))?
        } else {
            PyString::new(py, piece).into_any()
        };
        let kwargs = PyDict::new(py);
        kwargs.set_item("trim_url_limit", limit)?;
        kwargs.set_item("nofollow", true)?;
        kwargs.set_item("autoescape", autoescape)?;
        let link: String = urlizer.call((word,), Some(&kwargs))?.str()?.extract()?;
        output.push_str(&link);
        links.insert(piece, link);
    }
    Ok(output)
}

impl ResolveFilter for UrlizeFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        if let Some(text) = Text::from_variable(variable.as_ref(), py)? {
            let safe = matches!(text, Text::Safe(_));
            let output = urlize_text(
                text.as_str(),
                safe,
                None,
                context.autoescape,
                context.django_series(py)?,
                py,
            )?;
            return Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
                output,
            )))));
        }
        self.django.resolve(variable, py, template, context)
    }
}

impl ResolveFilter for UrlizetruncFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(limit) = argument.value().and_then(integer_argument)
            && let Some(text) = Text::from_variable(variable.as_ref(), py)?
        {
            let safe = matches!(text, Text::Safe(_));
            let output = urlize_text(
                text.as_str(),
                safe,
                Some(limit),
                context.autoescape,
                context.django_series(py)?,
                py,
            )?;
            return Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
                output,
            )))));
        }
        self.django.call(variable, argument, py, context)
    }
}

/// Appends `text` to `output` with Django's `normalize_newlines` and the
/// `<br>` replacement that `linebreaks` and `linebreaksbr` share: each
/// `\r\n`, `\r` and `\n` becomes `<br>`, and `escape` applies Django's
/// `escape` to the rest. With `paragraphs`, a run of two or more newlines
/// instead closes the paragraph, as `linebreaks` splits on `\n{2,}`.
fn push_linebreaks(output: &mut String, text: &str, escape: bool, paragraphs: bool) {
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let end = index;
        index += 1;
        let replacement = match byte {
            b'\n' | b'\r' => {
                index = end;
                let mut newlines = 0;
                while let Some(&newline @ (b'\n' | b'\r')) = bytes.get(index) {
                    index += 1;
                    if newline == b'\r' && bytes.get(index) == Some(&b'\n') {
                        index += 1;
                    }
                    newlines += 1;
                    if !paragraphs {
                        break;
                    }
                }
                if newlines > 1 { "</p>\n\n<p>" } else { "<br>" }
            }
            b'&' if escape => "&amp;",
            b'<' if escape => "&lt;",
            b'>' if escape => "&gt;",
            b'"' if escape => "&quot;",
            b'\'' if escape => "&#x27;",
            _ => continue,
        };
        output.push_str(&text[start..end]);
        output.push_str(replacement);
        start = index;
    }
    output.push_str(&text[start..]);
}

impl ResolveFilter for LinebreaksFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let (text, safe) = string_filter_text(variable, py)?;
        let mut output = String::with_capacity(text.len() + 24);
        output.push_str("<p>");
        push_linebreaks(&mut output, &text, context.autoescape && !safe, true);
        output.push_str("</p>");
        Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
            output,
        )))))
    }
}

impl ResolveFilter for LinebreaksbrFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let (text, safe) = string_filter_text(variable, py)?;
        let escape = context.autoescape && !safe;
        let changes = |byte: &u8| match byte {
            b'\n' | b'\r' => true,
            b'&' | b'<' | b'>' | b'"' | b'\'' => escape,
            _ => false,
        };
        // Text that needs no change is returned without copying.
        let text = match text.as_bytes().iter().position(changes) {
            None => text,
            Some(first) => {
                let mut output = String::with_capacity(text.len() + 16);
                output.push_str(&text[..first]);
                push_linebreaks(&mut output, &text[first..], escape, false);
                Cow::Owned(output)
            }
        };
        Ok(Some(Content::String(ContentString::HtmlSafe(text))))
    }
}

impl ResolveFilter for LinenumbersFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let (text, safe) = string_filter_text(variable, py)?;
        let escape = context.autoescape && !safe;
        // Django splits on "\n" only and zero-pads every number to the width
        // of the line count.
        let count = text.split('\n').count();
        let width = count.to_string().len();
        let mut output = String::with_capacity(text.len() + count * (width + 2));
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                output.push('\n');
            }
            let number = (index + 1).to_string();
            output.extend(std::iter::repeat_n('0', width - number.len()));
            output.push_str(&number);
            output.push_str(". ");
            if escape {
                encode_quoted_attribute_to_string(line, &mut output);
            } else {
                output.push_str(line);
            }
        }
        Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
            output,
        )))))
    }
}

static TITLE_FILTER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Django's `title` for ASCII text: Python's `str.title()`, then a capital
/// that follows a lowercase letter and an apostrophe, or a digit, is made
/// lowercase again.
fn title_ascii(text: &str) -> String {
    let mut title = Vec::with_capacity(text.len());
    let mut previous_cased = false;
    for byte in text.bytes() {
        title.push(if previous_cased {
            byte.to_ascii_lowercase()
        } else {
            byte.to_ascii_uppercase()
        });
        previous_cased = byte.is_ascii_alphabetic();
    }
    // `re.sub("([a-z])'([A-Z])", lower, ...)`. Matches don't overlap, so the
    // letter one match ends with can't start the next.
    let mut index = 0;
    while index + 2 < title.len() {
        if title[index].is_ascii_lowercase()
            && title[index + 1] == b'\''
            && title[index + 2].is_ascii_uppercase()
        {
            title[index + 2].make_ascii_lowercase();
            index += 3;
        } else {
            index += 1;
        }
    }
    // `re.sub(r"\d([A-Z])", lower, ...)`.
    let mut index = 0;
    while index + 1 < title.len() {
        if title[index].is_ascii_digit() && title[index + 1].is_ascii_uppercase() {
            title[index + 1].make_ascii_lowercase();
            index += 2;
        } else {
            index += 1;
        }
    }
    String::from_utf8_lossy(&title).into_owned()
}

impl ResolveFilter for TitleFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(content) = variable else {
            return Ok(Some("".as_content()));
        };
        let content_string = content.resolve_string(context)?;
        let text = content_string.as_raw();
        let title = if text.is_ascii() {
            title_ascii(text)
        } else {
            // Rust has no titlecase mapping (`ß` is `Ss`, `ǆ` is `ǅ`) and
            // lowercases a final sigma differently, so Python does these.
            TITLE_FILTER
                .import(py, "django.template.defaultfilters", "title")?
                .call1((text.as_ref(),))?
                .extract::<String>()?
        };
        Ok(Some(content_string.map_content(|_| Cow::Owned(title))))
    }
}

impl ResolveFilter for UpperFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        _py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let content = match variable {
            Some(content) => {
                let content = content.resolve_string(context)?;
                let uppercase = Cow::Owned(content.as_raw().to_uppercase());
                Content::String(if context.autoescape {
                    ContentString::HtmlUnsafe(uppercase)
                } else {
                    ContentString::String(uppercase)
                })
            }
            None => "".as_content(),
        };
        Ok(Some(content))
    }
}

impl ResolveFilter for WordcountFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        _py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let content = match variable {
            Some(content) => {
                let content_string = content.resolve_string(context)?;
                let word_count = content_string.as_raw().split_whitespace().count();
                Content::Int(word_count.into())
            }
            None => Content::Int(0.into()),
        };
        Ok(Some(content))
    }
}

/// A word-wrap function that preserves existing line breaks.
/// Expects that existing line breaks are posix newlines.
///
/// Preserve all white space except added line breaks consume the space on
/// which they break the line.
///
/// Don't wrap long words, thus the output text may have lines longer than ``width``.
fn wordwrap(text: &str, width: usize) -> String {
    let mut lines: Vec<Cow<str>> = Vec::new();
    for line in python_splitlines(text) {
        let before = lines.len();
        wrap_line(&expand_tabs(line), width, &mut lines);
        if lines.len() == before {
            // The line only held whitespace, which wrapping drops.
            lines.push(Cow::Borrowed(line));
        }
    }
    if text.ends_with('\n') {
        lines.push(Cow::Borrowed(""));
    }
    lines.join("\n")
}

/// Python's `str.isspace()` for one character.
fn is_python_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\x1c'..='\x1f')
}

/// Python's `str.splitlines()`.
fn python_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((index, c)) = characters.next() {
        let boundary = matches!(
            c,
            '\n' | '\r' | '\x0b' | '\x0c' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}'
        );
        if !boundary {
            continue;
        }
        lines.push(&text[start..index]);
        start = index + c.len_utf8();
        if c == '\r'
            && let Some((_, '\n')) = characters.peek()
        {
            characters.next();
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Python's `str.expandtabs()` for a single line: tab stops every 8 columns.
fn expand_tabs(line: &str) -> Cow<'_, str> {
    if !line.contains('\t') {
        return Cow::Borrowed(line);
    }
    let mut expanded = String::with_capacity(line.len() + 8);
    let mut column = 0;
    for c in line.chars() {
        if c == '\t' {
            let spaces = 8 - column % 8;
            expanded.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            expanded.push(c);
            column += 1;
        }
    }
    Cow::Owned(expanded)
}

/// `textwrap.TextWrapper(width, break_long_words=False,
/// break_on_hyphens=False, replace_whitespace=False).wrap(line)`, appended
/// to `lines`. The line is split into words and runs of ASCII whitespace.
/// Each output line takes the chunks that fit, without the whitespace at its
/// start (after the first line) and end; a word longer than `width` gets a
/// line to itself.
fn wrap_line(line: &str, width: usize, lines: &mut Vec<Cow<'_, str>>) {
    let is_separator = |c: char| matches!(c, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ');
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut in_whitespace = None;
    for (index, c) in line.char_indices() {
        let whitespace = is_separator(c);
        if in_whitespace.is_some_and(|previous| previous != whitespace) {
            chunks.push(&line[start..index]);
            start = index;
        }
        in_whitespace = Some(whitespace);
    }
    if start < line.len() {
        chunks.push(&line[start..]);
    }
    let is_blank = |chunk: &str| chunk.chars().all(is_python_space);
    let first_line = lines.len();
    let mut index = 0;
    while index < chunks.len() {
        let mut current: Vec<&str> = Vec::new();
        let mut length = 0_usize;
        if is_blank(chunks[index]) && lines.len() > first_line {
            index += 1;
        }
        while let Some(chunk) = chunks.get(index) {
            let chunk_length = chunk.chars().count();
            if length.saturating_add(chunk_length) > width {
                break;
            }
            current.push(chunk);
            length += chunk_length;
            index += 1;
        }
        if current.is_empty()
            && let Some(chunk) = chunks.get(index)
        {
            // A word longer than the width isn't broken.
            current.push(chunk);
            index += 1;
        }
        if current.last().is_some_and(|chunk| is_blank(chunk)) {
            current.pop();
        }
        if !current.is_empty() {
            lines.push(Cow::Owned(current.concat()));
        }
    }
}

impl ResolveFilter for WordwrapFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Django resolves a filter's arguments before calling it, so a
        // missing argument raises even for a missing value.
        let argument = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?;
        let Some(content) = variable else {
            return Ok(Some("".as_content()));
        };

        let text = content.resolve_string(context)?;

        let arg = argument.expect("missing argument in context should already have raised");

        // `textwrap` checks the width when it wraps a line, and empty text
        // has no lines.
        if !text.as_raw().is_empty()
            && let Some(n) = arg.to_bigint()
            && n <= BigInt::ZERO
        {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "invalid width {n} (must be > 0)"
            ))
            .annotate(py, self.argument.at, "width", template)
            .into());
        }

        let width = match arg.resolve_usize(self.argument.at) {
            Ok(w) => w,
            Err(PyRenderError::RenderError(RenderError::OverflowError { .. })) => usize::MAX,
            Err(e) => return Err(annotate_argument(e, py, self.argument.at, template)),
        };

        let wrapped = wordwrap(text.as_raw(), width);
        Ok(Some(text.map_content(|_| Cow::Owned(wrapped))))
    }
}

impl ResolveFilter for YesnoFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let arg_string = match &self.argument {
            Some(arg) => {
                let arg_content = arg
                    .resolve(py, template, context, ResolveFailures::Raise)?
                    .expect("missing argument in context should already have raised");
                match arg_content {
                    // Django uses its default for an argument of `None`.
                    Content::Py(value) if value.is_none() => {
                        Cow::Owned(gettext(py, "yes,no,maybe")?)
                    }
                    // `arg.split(",")` works on a lazy string too.
                    Content::Py(value) if is_lazy_string(&value)? => {
                        Cow::Owned(value.str()?.extract()?)
                    }
                    arg_content => {
                        string_argument(arg_content, context, (py, arg.at, template), |name| {
                            PyAttributeError::new_err(format!(
                                "'{name}' object has no attribute 'split'"
                            ))
                        })?
                    }
                }
            }
            None => Cow::Owned(gettext(py, "yes,no,maybe")?),
        };

        let bits: Vec<&str> = arg_string.split(',').collect();
        let (yes, no, maybe) = match bits.as_slice() {
            // If less than 2 values, return the original value
            [] | [_] => return Ok(variable),
            [yes, no] => (yes, no, no),
            [yes, no, maybe, ..] => (yes, no, maybe),
        };

        let result = match variable {
            Some(Content::Py(ref obj)) if obj.is_none() => maybe,
            Some(content) => match content.to_bool() {
                Ok(true) => yes,
                Ok(false) => no,
                Err(error) => {
                    let error = error.annotate(py, self.at, "when calling __bool__ here", template);
                    return Err(error.into());
                }
            },
            None => no,
        };

        Ok(Some(result.to_string().into_content()))
    }
}
