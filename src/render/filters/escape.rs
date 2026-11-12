//! Escaping filters: `escape`, `force_escape`, `escapejs`, `safe` and `json_script`.

use std::borrow::Cow;

use html_escape::encode_quoted_attribute_to_string;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyString};

use super::number::{python_float_repr, python_float_str};
use super::python::ResolvedArgument;
use super::sequence::exact_sequence;
use super::{CONDITIONAL_ESCAPE, ResolveFilter, SAFESTRING};
use crate::filters::{
    EscapeFilter, EscapejsFilter, ForceEscapeFilter, JsonScriptFilter, SafeFilter,
};
use crate::render::ResolveResult;
use crate::render::types::{AsBorrowedContent, Content, ContentString, Context, append_escaped};
use dtl_lexer::types::TemplateString;

impl ResolveFilter for EscapeFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        _context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        Ok(Some(Content::String(ContentString::HtmlSafe(
            match variable {
                Some(content) => match content {
                    Content::String(ContentString::HtmlSafe(content)) => content,
                    Content::String(content) => {
                        let mut encoded = String::new();
                        encode_quoted_attribute_to_string(content.as_raw(), &mut encoded);
                        Cow::Owned(encoded)
                    }
                    Content::Int(n) => Cow::Owned(n.to_string()),
                    Content::Float(n) => Cow::Owned(python_float_str(n)),
                    // `escape` is a `stringfilter`: `conditional_escape` sees
                    // `str(value)`, which keeps a `SafeString` from `__str__`.
                    Content::Py(object) => {
                        let text = object.str()?;
                        if text.is_exact_instance_of::<PyString>() {
                            let mut encoded = String::new();
                            append_escaped(&text.to_cow()?, &mut encoded);
                            Cow::Owned(encoded)
                        } else {
                            let escaped = CONDITIONAL_ESCAPE
                                .import(py, "django.utils.html", "conditional_escape")?
                                .call1((text,))?;
                            Cow::Owned(escaped.extract::<String>()?)
                        }
                    }
                    Content::Bool(true) => Cow::Borrowed("True"),
                    Content::Bool(false) => Cow::Borrowed("False"),
                },
                None => Cow::Borrowed(""),
            },
        ))))
    }
}

/// Hex encode characters for use in JavaScript strings.
fn escapejs(value: &str, escape_c1: bool) -> String {
    let mut result = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => result.push_str(r"\u005C"),
            '\'' => result.push_str(r"\u0027"),
            '"' => result.push_str(r"\u0022"),
            '>' => result.push_str(r"\u003E"),
            '<' => result.push_str(r"\u003C"),
            '&' => result.push_str(r"\u0026"),
            '=' => result.push_str(r"\u003D"),
            '-' => result.push_str(r"\u002D"),
            ';' => result.push_str(r"\u003B"),
            '`' => result.push_str(r"\u0060"),
            // Line separator
            '\u{2028}' => result.push_str(r"\u2028"),
            // Paragraph Separator
            '\u{2029}' => result.push_str(r"\u2029"),
            // c as u32 is safe because all chars are valid u32
            // See https://doc.rust-lang.org/std/primitive.char.html#method.from_u32
            c if matches!(c, '\0'..='\x1f') || (escape_c1 && matches!(c, '\u{7f}'..='\u{9f}')) => {
                result.push_str(&format!(r"\u{:04X}", c as u32));
            }
            c => result.push(c),
        }
    }
    result
}

impl ResolveFilter for EscapejsFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let series = context.django_series(py)?;
        let content = match variable {
            Some(content) => content
                .resolve_string(context)?
                .map_content(|content| Cow::Owned(escapejs(&content, series >= (6, 1)))),
            None => "".as_content(),
        };
        Ok(Some(content))
    }
}

/// Whether `object` is exactly a `SafeString`, so `conditional_escape`
/// returns it as it is.
pub fn is_exact_safe_string(object: &Bound<'_, PyAny>) -> PyResult<bool> {
    let py = object.py();
    Ok(object
        .get_type()
        .is(SAFESTRING.import(py, "django.utils.safestring", "SafeString")?))
}

impl ResolveFilter for ForceEscapeFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let variable = match variable {
            // We want to re-run the escape filter even if the content has already been resolved to safe HTML
            Some(Content::String(ContentString::HtmlSafe(c))) => {
                Some(Content::String(ContentString::String(c)))
            }
            Some(Content::Py(value)) => Some(Content::String(ContentString::String(Cow::Owned(
                value.str()?.extract::<String>()?,
            )))),
            other => other,
        };
        EscapeFilter.resolve(variable, py, template, context)
    }
}

impl ResolveFilter for SafeFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        _py: Python<'py>,
        _template: TemplateString<'t>,
        _context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        Ok(Some(Content::String(ContentString::HtmlSafe(
            match variable {
                Some(content) => match content {
                    Content::String(content) => content.into_raw(),
                    Content::Int(n) => Cow::Owned(n.to_string()),
                    Content::Float(n) => Cow::Owned(python_float_str(n)),
                    Content::Py(object) => {
                        let content = object.str()?.extract::<String>()?;
                        Cow::Owned(content)
                    }
                    Content::Bool(true) => Cow::Borrowed("True"),
                    Content::Bool(false) => Cow::Borrowed("False"),
                },
                None => Cow::Borrowed(""),
            },
        ))))
    }
}

/// JSON nesting beyond this goes to Django, which detects circular
/// references.
const MAX_JSON_DEPTH: usize = 100;

/// A JSON string as `json.dumps` writes it with `ensure_ascii`, followed by
/// `json_script`'s escapes for `<`, `>` and `&`.
fn push_json_string(text: &str, output: &mut String) {
    use std::fmt::Write;
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            '<' => output.push_str("\\u003C"),
            '>' => output.push_str("\\u003E"),
            '&' => output.push_str("\\u0026"),
            ' '..='~' => output.push(character),
            _ => {
                let mut units = [0; 2];
                for unit in character.encode_utf16(&mut units) {
                    let _ = write!(output, "\\u{unit:04x}");
                }
            }
        }
    }
    output.push('"');
}

/// Writes `value` as `json.dumps` would with Django's encoder. Returns
/// `false` for values left to Django: subclasses, lazy strings, dates,
/// decimals, other keys and deep nesting.
fn push_json(value: &Bound<'_, PyAny>, depth: usize, output: &mut String) -> PyResult<bool> {
    if depth > MAX_JSON_DEPTH {
        return Ok(false);
    }
    if value.is_none() {
        output.push_str("null");
    } else if value.is_exact_instance_of::<PyBool>() {
        output.push_str(if value.is_truthy()? { "true" } else { "false" });
    } else if value.is_exact_instance_of::<PyInt>() {
        output.push_str(&value.str()?.to_cow()?);
    } else if value.is_exact_instance_of::<PyFloat>() {
        let number = value.extract::<f64>()?;
        output.push_str(&if number.is_nan() {
            "NaN".to_owned()
        } else if number.is_infinite() {
            if number > 0.0 {
                "Infinity"
            } else {
                "-Infinity"
            }
            .to_owned()
        } else {
            python_float_repr(number)
        });
    } else if value.is_exact_instance_of::<PyString>() || is_exact_safe_string(value)? {
        let Ok(text) = value.cast::<PyString>()?.to_str() else {
            return Ok(false);
        };
        push_json_string(text, output);
    } else if let Some(items) = exact_sequence(value) {
        output.push('[');
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            if !push_json(item, depth + 1, output)? {
                return Ok(false);
            }
        }
        output.push(']');
    } else if value.is_exact_instance_of::<PyDict>() {
        output.push('{');
        for (index, (key, item)) in value.cast::<PyDict>()?.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            if key.is_exact_instance_of::<PyString>() {
                let Ok(text) = key.cast::<PyString>()?.to_str() else {
                    return Ok(false);
                };
                push_json_string(text, output);
            } else if key.is_exact_instance_of::<PyInt>() {
                output.push('"');
                output.push_str(&key.str()?.to_cow()?);
                output.push('"');
            } else {
                return Ok(false);
            }
            output.push_str(": ");
            if !push_json(&item, depth + 1, output)? {
                return Ok(false);
            }
        }
        output.push('}');
    } else {
        return Ok(false);
    }
    Ok(true)
}

impl ResolveFilter for JsonScriptFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        // `format_html` escapes the id unless it is safe, as literals are;
        // a falsy id is left out.
        let element_id = match argument.value() {
            None => Some(None),
            Some(Content::String(ContentString::HtmlSafe(text) | ContentString::String(text))) => {
                Some(Some((text.as_ref(), false)))
            }
            Some(Content::String(ContentString::HtmlUnsafe(text))) => {
                Some(Some((text.as_ref(), true)))
            }
            Some(Content::Py(object)) if object.is_exact_instance_of::<PyString>() => {
                object.extract::<&str>().ok().map(|text| Some((text, true)))
            }
            Some(Content::Py(object)) if is_exact_safe_string(object)? => object
                .extract::<&str>()
                .ok()
                .map(|text| Some((text, false))),
            Some(_) => None,
        };
        if let Some(element_id) = element_id
            && let Some(Content::Py(value)) = &variable
        {
            let mut output = String::from("<script ");
            if let Some((id, escape)) = element_id.filter(|(id, _)| !id.is_empty()) {
                output.push_str("id=\"");
                if escape {
                    append_escaped(id, &mut output);
                } else {
                    output.push_str(id);
                }
                output.push_str("\" ");
            }
            output.push_str("type=\"application/json\">");
            if push_json(value, 0, &mut output)? {
                output.push_str("</script>");
                return Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
                    output,
                )))));
            }
        }
        // Literal ids are safe strings in Django, even with autoescape off.
        let argument = match argument {
            ResolvedArgument::Present(Some(Content::String(ContentString::String(text)))) => {
                let safe = SAFESTRING
                    .import(py, "django.utils.safestring", "SafeString")?
                    .call1((text.as_ref(),))?;
                ResolvedArgument::Present(Some(Content::Py(safe)))
            }
            argument => argument,
        };
        self.django.call(variable, argument, py, context)
    }
}
