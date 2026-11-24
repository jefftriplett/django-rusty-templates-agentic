//! Small helpers that several filter modules share.

use pyo3::prelude::*;
use pyo3::types::{PyList, PyString, PyTuple};

use super::SAFESTRING;
use crate::render::types::{Content, ContentString};

/// Whether `object` is exactly a `SafeString`, so `conditional_escape`
/// returns it as it is.
pub fn is_exact_safe_string(object: &Bound<'_, PyAny>) -> PyResult<bool> {
    let py = object.py();
    Ok(object
        .get_type()
        .is(SAFESTRING.import(py, "django.utils.safestring", "SafeString")?))
}

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

/// Whitespace as Python's `str.split()` sees it: Unicode `White_Space` plus
/// the information separators U+001C to U+001F.
pub(super) fn is_python_whitespace(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
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
