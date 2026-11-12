//! Filters over sequences: `first`, `last`, `join`, `slice`, `dictsort` and friends.

use std::borrow::Cow;

use html_escape::encode_quoted_attribute_to_string;
use num_traits::ToPrimitive;
use pyo3::exceptions::{PyAttributeError, PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyDict, PyFloat, PyInt, PyList, PyRange, PySlice, PyString, PyTuple};

use super::escape::is_exact_safe_string;
use super::text::format_spec_text;
use super::{
    CONDITIONAL_ESCAPE, MARK_FILTER_SAFE, ResolveFilter, SAFEDATA, SAFESTRING, string_filter_text,
};
use crate::error::AnnotatePyErr;
use crate::filters::{
    DictsortFilter, DictsortreversedFilter, EscapeseqFilter, FirstFilter, JoinFilter, LastFilter,
    LengthFilter, MakeListFilter, RandomFilter, SafeseqFilter, SliceArgument, SliceFilter,
    UnorderedListFilter,
};
use crate::render::ResolveResult;
use crate::render::types::{
    AsBorrowedContent, Content, ContentString, Context, append_escaped, append_python_string,
};
use dtl_lexer::types::TemplateString;

/// What Django's `join` returns for a list or tuple.
enum Joined {
    /// `mark_safe` of the joined text.
    Text(String),
    /// The value itself: joining raised `TypeError`, which Django's filter
    /// swallows.
    Unchanged,
}

/// The separator Django's `join` writes: `conditional_escape(arg)` when
/// autoescaping, otherwise `arg` itself. `None` leaves other separators to
/// Django.
fn join_separator(separator: &Content<'_, '_>, autoescape: bool) -> PyResult<Option<String>> {
    let (text, safe) = match separator {
        Content::String(ContentString::HtmlSafe(text)) => (text.as_ref(), true),
        Content::String(ContentString::String(text) | ContentString::HtmlUnsafe(text)) => {
            (text.as_ref(), false)
        }
        Content::Py(object) if object.is_exact_instance_of::<PyString>() => {
            (object.cast::<PyString>()?.to_str()?, false)
        }
        Content::Py(object) if is_exact_safe_string(object)? => {
            (object.cast::<PyString>()?.to_str()?, true)
        }
        Content::Py(_) | Content::Int(_) | Content::Float(_) | Content::Bool(_) => {
            return Ok(None);
        }
    };
    let mut output = String::with_capacity(text.len());
    if autoescape && !safe {
        append_escaped(text, &mut output);
    } else {
        output.push_str(text);
    }
    Ok(Some(output))
}

/// Django's `join` over the items of a list or tuple.
fn join_items<'py>(
    value: &Bound<'py, PyAny>,
    separator: &str,
    autoescape: bool,
    py: Python<'py>,
) -> PyResult<Joined> {
    let mut output = String::new();
    // Django catches a `TypeError` from iterating, as for a non-iterable.
    let items = match value.try_iter() {
        Err(error) if error.is_instance_of::<PyTypeError>(py) => return Ok(Joined::Unchanged),
        items => items?,
    };
    for (index, item) in items.enumerate() {
        let item = match item {
            Err(error) if error.is_instance_of::<PyTypeError>(py) => {
                return Ok(Joined::Unchanged);
            }
            item => item?,
        };
        if index > 0 {
            output.push_str(separator);
        }
        if !autoescape {
            // `str.join` accepts only strings.
            if !item.is_instance_of::<PyString>() {
                return Ok(Joined::Unchanged);
            }
            append_python_string(&item, false, &mut output)?;
        } else if item.is_exact_instance_of::<PyString>() {
            append_python_string(&item, true, &mut output)?;
        } else if is_exact_safe_string(&item)? {
            append_python_string(&item, false, &mut output)?;
        } else if item.is_exact_instance_of::<PyInt>() {
            // Digits need no escaping.
            append_python_string(&item.str()?.into_any(), false, &mut output)?;
        } else {
            let escaped = match CONDITIONAL_ESCAPE
                .import(py, "django.utils.html", "conditional_escape")?
                .call1((item,))
            {
                Err(error) if error.is_instance_of::<PyTypeError>(py) => {
                    return Ok(Joined::Unchanged);
                }
                result => result?,
            };
            // `str.join` raises `TypeError` for an `__html__` that returns a non-string.
            if !escaped.is_instance_of::<PyString>() {
                return Ok(Joined::Unchanged);
            }
            append_python_string(&escaped, false, &mut output)?;
        }
    }
    Ok(Joined::Text(output))
}

impl ResolveFilter for JoinFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(separator) = argument.value()
            && let Some(separator) = join_separator(separator, context.autoescape)?
        {
            // Any value goes through Python's iteration, as in Django: strings
            // join their characters and non-iterables are returned unchanged.
            let value = match &variable {
                Some(value) => value.to_py(py),
                // Django's filter gets "".
                None => PyString::new(py, "").into_any(),
            };
            let joined = join_items(&value, &separator, context.autoescape, py)?;
            return Ok(Some(match joined {
                Joined::Text(text) => Content::String(ContentString::HtmlSafe(Cow::Owned(text))),
                Joined::Unchanged => variable.unwrap_or_else(|| "".as_content()),
            }));
        }
        self.django.call(variable, argument, py, context)
    }
}

impl ResolveFilter for FirstFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        _context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(content) = variable else {
            return Ok(Some("".as_content()));
        };
        // Django's `first` returns `value[0]`, or "" on an `IndexError`.
        // Indexing a string, even a safe one, gives a plain `str`.
        if let Content::String(text) = &content {
            let first = text.as_raw().chars().next().map(String::from);
            return Ok(Some(Content::String(ContentString::HtmlUnsafe(
                first.map_or(Cow::Borrowed(""), Cow::Owned),
            ))));
        }
        match content.to_py(py).get_item(0) {
            Ok(first) => Ok(Some(Content::Py(first))),
            Err(error) if error.is_instance_of::<pyo3::exceptions::PyIndexError>(py) => {
                Ok(Some("".as_content()))
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl ResolveFilter for LastFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        _context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(content) = variable else {
            return Ok(Some("".as_content()));
        };

        let obj = content.to_py(py);

        match obj.get_item(-1) {
            // `is_safe=True`: the last character of safe text is safe.
            Ok(last)
                if last.is_exact_instance_of::<PyString>()
                    && obj.is_instance(SAFEDATA.import(
                        py,
                        "django.utils.safestring",
                        "SafeData",
                    )?)? =>
            {
                let last = last.extract::<String>()?;
                Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
                    last,
                )))))
            }
            Ok(last) => Ok(Some(Content::Py(last))),
            Err(err) => {
                if err.is_instance_of::<pyo3::exceptions::PyIndexError>(py) {
                    Ok(Some("".as_content()))
                } else {
                    let error = err.annotate(py, self.at, "here", template);
                    Err(error.into())
                }
            }
        }
    }
}

impl ResolveFilter for LengthFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        _py: Python<'py>,
        _template: TemplateString<'t>,
        _context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let len = match variable {
            None => 0,
            Some(Content::String(s)) => s.as_raw().chars().count(),
            Some(Content::Py(obj)) => obj.len().unwrap_or(0),
            Some(Content::Int(_) | Content::Float(_) | Content::Bool(_)) => 0,
        };

        Ok(Some(Content::Int(num_bigint::BigInt::from(len))))
    }
}

/// The slice Django builds from a resolved argument. `None` leaves unusual
/// arguments to Django's filter.
fn slice_argument(argument: Option<&Content<'_, '_>>) -> Option<SliceArgument> {
    match argument {
        // Django's filter receives `None`, and `int("None")` fails.
        None | Some(Content::Float(_) | Content::Bool(_)) => Some(SliceArgument::Invalid),
        Some(Content::Int(n)) => n.to_isize().map(SliceArgument::stop),
        Some(Content::String(text)) => SliceArgument::parse(text.as_raw()),
        Some(Content::Py(object)) if object.is_exact_instance_of::<PyInt>() => {
            object.extract().ok().map(SliceArgument::stop)
        }
        Some(Content::Py(object)) if object.is_exact_instance_of::<PyString>() => {
            SliceArgument::parse(object.cast::<PyString>().ok()?.to_str().ok()?)
        }
        // Django parses `str(arg)`.
        Some(Content::Py(object)) => SliceArgument::parse(object.str().ok()?.to_str().ok()?),
    }
}

/// Python's `slice(start, stop, step).indices(length)`, as the first index,
/// the step and the number of items selected.
fn slice_indices(
    length: usize,
    start: Option<isize>,
    stop: Option<isize>,
    step: isize,
) -> (isize, isize, usize) {
    let length = isize::try_from(length).unwrap_or(isize::MAX);
    // Keep the step negatable, as CPython does.
    let step = step.max(-isize::MAX);
    let adjust = |index: Option<isize>, default: isize| {
        let index = index.unwrap_or(default);
        if index < 0 {
            let index = index + length;
            if index < 0 {
                if step < 0 { -1 } else { 0 }
            } else {
                index
            }
        } else if index >= length {
            if step < 0 { length - 1 } else { length }
        } else {
            index
        }
    };
    let (start, stop) = if step < 0 {
        (adjust(start, isize::MAX), adjust(stop, isize::MIN))
    } else {
        (adjust(start, 0), adjust(stop, isize::MAX))
    };
    let count = if step < 0 && stop < start {
        (start - stop - 1) / -step + 1
    } else if step > 0 && start < stop {
        (stop - start - 1) / step + 1
    } else {
        0
    };
    (start, step, count.unsigned_abs())
}

/// Slices `text` by characters, as Python slices a `str`, for a non-zero
/// step. A step of one borrows from `text`.
fn slice_text(
    text: Cow<'_, str>,
    start: Option<isize>,
    stop: Option<isize>,
    step: Option<isize>,
) -> Cow<'_, str> {
    let step = step.unwrap_or(1);
    let ascii = text.is_ascii();
    let length = if ascii {
        text.len()
    } else {
        text.chars().count()
    };
    let (first, step, count) = slice_indices(length, start, stop, step);
    if count == 0 {
        return Cow::Borrowed("");
    }
    let first = first.unsigned_abs();
    if step == 1 {
        let byte = |index: usize| match ascii {
            true => index,
            false => text
                .char_indices()
                .nth(index)
                .map_or(text.len(), |(at, _)| at),
        };
        let range = byte(first)..byte(first + count);
        return match text {
            Cow::Borrowed(text) => Cow::Borrowed(&text[range]),
            Cow::Owned(text) => Cow::Owned(text[range].to_string()),
        };
    }
    let stride = step.unsigned_abs();
    Cow::Owned(if step > 0 {
        text.chars()
            .skip(first)
            .step_by(stride)
            .take(count)
            .collect()
    } else {
        text.chars()
            .rev()
            .skip(length - 1 - first)
            .step_by(stride)
            .take(count)
            .collect()
    })
}

impl ResolveFilter for SliceFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let slice = match self.literal {
            Some(slice) => slice,
            None => {
                let argument = self.django.resolve_argument(py, template, context)?;
                match slice_argument(argument.value()) {
                    Some(slice) => slice,
                    None => return self.django.call(variable, argument, py, context),
                }
            }
        };
        let Some(value) = variable else {
            return Ok(Some("".as_content()));
        };
        let SliceArgument::Bounds { start, stop, step } = slice else {
            return Ok(Some(value));
        };
        Ok(Some(match value {
            // Python rejects a zero step with ValueError, so Django returns the value.
            Content::String(string) if step == Some(0) => Content::String(string),
            Content::String(string) => {
                let kind: fn(Cow<'t, str>) -> ContentString<'t> = match string {
                    ContentString::HtmlSafe(_) => ContentString::HtmlSafe,
                    ContentString::String(_) if !context.autoescape => ContentString::String,
                    ContentString::String(_) | ContentString::HtmlUnsafe(_) => {
                        ContentString::HtmlUnsafe
                    }
                };
                Content::String(kind(slice_text(string.into_raw(), start, stop, step)))
            }
            // Numbers are not subscriptable: Django catches the TypeError.
            Content::Int(_) | Content::Float(_) | Content::Bool(_) => value,
            Content::Py(object) => {
                let slice = py.get_type::<PySlice>().call1((start, stop, step))?;
                match object.get_item(slice) {
                    Ok(sliced) => {
                        let safe = SAFEDATA.import(py, "django.utils.safestring", "SafeData")?;
                        Content::Py(if object.is_instance(safe)? {
                            MARK_FILTER_SAFE
                                .import(py, "django.utils.safestring", "mark_safe")?
                                .call1((sliced,))?
                        } else {
                            sliced
                        })
                    }
                    Err(error)
                        if error.is_instance_of::<PyValueError>(py)
                            || error.is_instance_of::<PyTypeError>(py)
                            || error.is_instance_of::<PyKeyError>(py) =>
                    {
                        Content::Py(object)
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }))
    }
}

impl ResolveFilter for MakeListFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        _context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // `list(str(value))`: the characters are plain strings, even when the
        // value was safe.
        let (text, _) = string_filter_text(variable, py)?;
        let mut buffer = [0; 4];
        let characters = PyList::empty(py);
        for character in text.chars() {
            characters.append(character.encode_utf8(&mut buffer) as &str)?;
        }
        Ok(Some(Content::Py(characters.into_any())))
    }
}

/// The items of an exact list or tuple, which `escapeseq` and `safeseq`
/// walk natively. Other iterables are left to Django.
fn sequence_items<'py>(variable: Option<&Content<'_, 'py>>) -> Option<Vec<Bound<'py, PyAny>>> {
    let Some(Content::Py(value)) = variable else {
        return None;
    };
    if let Ok(list) = value.cast::<PyList>()
        && value.is_exact_instance_of::<PyList>()
    {
        Some(list.iter().collect())
    } else if let Ok(tuple) = value.cast::<PyTuple>()
        && value.is_exact_instance_of::<PyTuple>()
    {
        Some(tuple.iter().collect())
    } else {
        None
    }
}

impl ResolveFilter for EscapeseqFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(items) = sequence_items(variable.as_ref()) else {
            return self.django.resolve(variable, py, template, context);
        };
        let safe_string = SAFESTRING.import(py, "django.utils.safestring", "SafeString")?;
        let escaped = PyList::empty(py);
        for item in items {
            // `conditional_escape()`: plain strings are escaped, `SafeString`
            // is its own `__html__()`, and other items go to Django.
            if item.is_exact_instance_of::<PyString>() {
                let mut text = String::new();
                encode_quoted_attribute_to_string(item.extract::<&str>()?, &mut text);
                escaped.append(safe_string.call1((text,))?)?;
            } else if item.get_type().is(safe_string) {
                escaped.append(item)?;
            } else {
                escaped.append(
                    CONDITIONAL_ESCAPE
                        .import(py, "django.utils.html", "conditional_escape")?
                        .call1((item,))?,
                )?;
            }
        }
        Ok(Some(Content::Py(escaped.into_any())))
    }
}

impl ResolveFilter for SafeseqFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(items) = sequence_items(variable.as_ref()) else {
            return self.django.resolve(variable, py, template, context);
        };
        let safe_string = SAFESTRING.import(py, "django.utils.safestring", "SafeString")?;
        let marked = PyList::empty(py);
        for item in items {
            // `mark_safe()`: plain strings become `SafeString`, values with
            // `__html__` such as `SafeString` stay as they are, and other
            // items go to Django.
            if item.is_exact_instance_of::<PyString>() {
                marked.append(safe_string.call1((item,))?)?;
            } else if item.get_type().is(safe_string) {
                marked.append(item)?;
            } else {
                marked.append(
                    MARK_FILTER_SAFE
                        .import(py, "django.utils.safestring", "mark_safe")?
                        .call1((item,))?,
                )?;
            }
        }
        Ok(Some(Content::Py(marked.into_any())))
    }
}

static RANDOM_CHOICE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

impl ResolveFilter for RandomFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Python's `random.choice` keeps `random.seed()` working; calling it
        // directly skips the general filter call. Django returns "" for an
        // empty sequence.
        if let Some(Content::Py(value)) = &variable
            && (value.is_exact_instance_of::<PyList>() || value.is_exact_instance_of::<PyTuple>())
        {
            if value.len()? == 0 {
                return Ok(Some("".as_content()));
            }
            let choice = RANDOM_CHOICE
                .import(py, "random", "choice")?
                .call1((value,))?;
            return Ok(Some(Content::Py(choice)));
        }
        self.django.resolve(variable, py, template, context)
    }
}

/// Nesting beyond this goes to Django, which raises `RecursionError` for
/// lists that contain themselves.
const MAX_LIST_DEPTH: usize = 100;

/// The items of an exact list or tuple. Subclasses and other iterables
/// return `None`, leaving them to Django.
pub(super) fn exact_sequence<'py>(value: &Bound<'py, PyAny>) -> Option<Vec<Bound<'py, PyAny>>> {
    if value.is_exact_instance_of::<PyList>() {
        value
            .cast::<PyList>()
            .ok()
            .map(|list| list.iter().collect())
    } else if value.is_exact_instance_of::<PyTuple>() {
        value
            .cast::<PyTuple>()
            .ok()
            .map(|tuple| tuple.iter().collect())
    } else {
        None
    }
}

/// Django's `list_formatter`: each item becomes an indented `<li>`, and a
/// list or tuple right after an item becomes its nested `<ul>`. Returns
/// `false` for structures left to Django.
fn push_list_items(
    items: &[Bound<'_, PyAny>],
    tabs: usize,
    autoescape: bool,
    output: &mut String,
) -> PyResult<bool> {
    if tabs > MAX_LIST_DEPTH {
        return Ok(false);
    }
    let py = match items.first() {
        Some(item) => item.py(),
        None => return Ok(true),
    };
    let generator = py.import("types")?.getattr("GeneratorType")?;
    let mut index = 0;
    let mut first = true;
    while let Some(item) = items.get(index) {
        index += 1;
        // Django pairs an item with a following list, tuple or generator.
        let children = match items.get(index) {
            Some(next) if next.is_instance_of::<PyList>() || next.is_instance_of::<PyTuple>() => {
                let Some(children) = exact_sequence(next) else {
                    return Ok(false);
                };
                index += 1;
                Some(children)
            }
            Some(next) if next.is_instance(&generator)? => return Ok(false),
            _ => None,
        };
        if !first {
            output.push('\n');
        }
        first = false;
        output.extend(std::iter::repeat_n('\t', tabs));
        output.push_str("<li>");
        if item.is_exact_instance_of::<PyString>() {
            append_python_string(item, autoescape, output)?;
        } else if is_exact_safe_string(item)? || item.is_exact_instance_of::<PyInt>() {
            append_python_string(&item.str()?.into_any(), false, output)?;
        } else if autoescape {
            let escaped = CONDITIONAL_ESCAPE
                .import(py, "django.utils.html", "conditional_escape")?
                .call1((item,))?;
            append_python_string(&escaped.str()?.into_any(), false, output)?;
        } else {
            append_python_string(&item.str()?.into_any(), false, output)?;
        }
        // An empty list is consumed but adds no `<ul>`.
        if let Some(children) = children.filter(|children| !children.is_empty()) {
            output.push('\n');
            output.extend(std::iter::repeat_n('\t', tabs));
            output.push_str("<ul>\n");
            if !push_list_items(&children, tabs + 1, autoescape, output)? {
                return Ok(false);
            }
            output.push('\n');
            output.extend(std::iter::repeat_n('\t', tabs));
            output.push_str("</ul>\n");
            output.extend(std::iter::repeat_n('\t', tabs));
        }
        output.push_str("</li>");
    }
    Ok(true)
}

impl ResolveFilter for UnorderedListFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        if let Some(Content::Py(value)) = &variable
            && let Some(items) = exact_sequence(value)
        {
            let mut output = String::new();
            if push_list_items(&items, 1, context.autoescape, &mut output)? {
                return Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
                    output,
                )))));
            }
        }
        self.django.resolve(variable, py, template, context)
    }
}

/// The dotted key path of `dictsort`, or `None` when Django resolves the
/// argument differently: numbers become `itemgetter`, and private names
/// and an empty argument raise.
fn dictsort_path(argument: &str) -> Option<Vec<&str>> {
    let number = argument.trim().replace('_', "");
    if argument.is_empty()
        || argument.starts_with('_')
        || argument.contains("._")
        || number.parse::<f64>().is_ok()
    {
        return None;
    }
    Some(argument.split('.').collect())
}

/// Whether a `dictsort` error makes Django's filter return `""`.
fn dictsort_failure(error: &PyErr, py: Python<'_>) -> bool {
    error.is_instance_of::<PyAttributeError>(py) || error.is_instance_of::<PyTypeError>(py)
}

/// Django's `_property_resolver`: each part is tried as an item, then as
/// an attribute. Returns `None` when Django's filter would return `""`.
fn dictsort_key<'py>(
    item: &Bound<'py, PyAny>,
    path: &[&str],
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let py = item.py();
    let mut value = item.clone();
    for part in path {
        if value.is_exact_instance_of::<PyDict>()
            && let Some(found) = value.cast::<PyDict>()?.get_item(part)?
        {
            value = found;
            continue;
        }
        value = match value.get_item(part) {
            Ok(found) => found,
            Err(error)
                if dictsort_failure(&error, py)
                    || error.is_instance_of::<PyIndexError>(py)
                    || error.is_instance_of::<PyKeyError>(py)
                    || error.is_instance_of::<PyValueError>(py) =>
            {
                match value.getattr(*part) {
                    Ok(found) => found,
                    Err(error) if dictsort_failure(&error, py) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
    }
    Ok(Some(value))
}

/// The order of `keys` that a stable sort gives: by Rust comparisons for
/// strings, integers and floats of one type, and by Python's `sorted`
/// otherwise. Returns `None` when Django's filter would return `""`.
fn dictsort_order(
    keys: &[Bound<'_, PyAny>],
    reverse: bool,
    py: Python<'_>,
) -> PyResult<Option<Vec<usize>>> {
    fn sort_by<T>(
        values: &[T],
        reverse: bool,
        compare: impl Fn(&T, &T) -> std::cmp::Ordering,
    ) -> Vec<usize> {
        let mut order: Vec<usize> = (0..values.len()).collect();
        if reverse {
            order.sort_by(|&a, &b| compare(&values[b], &values[a]));
        } else {
            order.sort_by(|&a, &b| compare(&values[a], &values[b]));
        }
        order
    }
    if keys
        .iter()
        .all(pyo3::types::PyAnyMethods::is_exact_instance_of::<PyString>)
        && let Ok(strings) = keys
            .iter()
            .map(|key| key.cast::<PyString>()?.to_str())
            .collect::<PyResult<Vec<_>>>()
    {
        return Ok(Some(sort_by(&strings, reverse, std::cmp::Ord::cmp)));
    }
    if keys
        .iter()
        .all(pyo3::types::PyAnyMethods::is_exact_instance_of::<PyInt>)
        && let Ok(ints) = keys
            .iter()
            .map(pyo3::types::PyAnyMethods::extract::<i64>)
            .collect::<PyResult<Vec<_>>>()
    {
        return Ok(Some(sort_by(&ints, reverse, Ord::cmp)));
    }
    if keys
        .iter()
        .all(pyo3::types::PyAnyMethods::is_exact_instance_of::<PyFloat>)
        && let Ok(floats) = keys
            .iter()
            .map(pyo3::types::PyAnyMethods::extract::<f64>)
            .collect::<PyResult<Vec<_>>>()
        && !floats.iter().any(|value| value.is_nan())
    {
        return Ok(Some(sort_by(&floats, reverse, |a, b| {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        })));
    }
    // Python compares the same keys in the same order as Django's call.
    let keys = PyList::new(py, keys)?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("key", keys.getattr("__getitem__")?)?;
    kwargs.set_item("reverse", reverse)?;
    let sorted = py.import("builtins")?.getattr("sorted")?.call(
        (PyRange::new(py, 0, isize::try_from(keys.len())?)?,),
        Some(&kwargs),
    );
    match sorted {
        Ok(order) => Ok(Some(order.extract()?)),
        Err(error) if dictsort_failure(&error, py) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Django's `dictsort` and `dictsortreversed` for exact lists and tuples
/// with a key path. Returns `None` to leave the call to Django.
fn dictsort<'t, 'py>(
    variable: Option<&Content<'t, 'py>>,
    argument: Option<&Content<'t, 'py>>,
    reverse: bool,
    py: Python<'py>,
) -> PyResult<Option<Content<'t, 'py>>> {
    let Some(Content::Py(value)) = variable else {
        return Ok(None);
    };
    let Some(items) = exact_sequence(value) else {
        return Ok(None);
    };
    let Some(path) = argument.and_then(format_spec_text).and_then(dictsort_path) else {
        return Ok(None);
    };
    let failed = || Some(Content::String(ContentString::String(Cow::Borrowed(""))));
    let mut keys = Vec::with_capacity(items.len());
    for item in &items {
        match dictsort_key(item, &path)? {
            Some(key) => keys.push(key),
            None => return Ok(failed()),
        }
    }
    let Some(order) = dictsort_order(&keys, reverse, py)? else {
        return Ok(failed());
    };
    let sorted = PyList::new(py, order.into_iter().map(|index| &items[index]))?;
    Ok(Some(Content::Py(sorted.into_any())))
}

impl ResolveFilter for DictsortFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(sorted) = dictsort(variable.as_ref(), argument.value(), false, py)? {
            return Ok(Some(sorted));
        }
        self.django.call(variable, argument, py, context)
    }
}

impl ResolveFilter for DictsortreversedFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(sorted) = dictsort(variable.as_ref(), argument.value(), true, py)? {
            return Ok(Some(sorted));
        }
        self.django.call(variable, argument, py, context)
    }
}
