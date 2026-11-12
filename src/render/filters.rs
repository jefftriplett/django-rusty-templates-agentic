use std::borrow::Cow;

use num_traits::ToPrimitive;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyFloat, PyInt, PyString, PyType};

use crate::error::{AnnotatePyErr, PyRenderError, RenderError};
use crate::filters::{DefaultFilter, DefaultIfNoneFilter, FilterType, python_int};
use crate::parse::Filter;
use crate::render::types::{AsBorrowedContent, Content, ContentString, Context};
use crate::render::{Resolve, ResolveFailures, ResolveResult};
use dtl_lexer::types::TemplateString;

mod date;
mod escape;
mod number;
mod python;
mod sequence;
mod text;
mod truncate;

pub use date::{DATE_FILTER, format_localtime};
pub(crate) use escape::is_exact_safe_string;
pub(crate) use number::{
    DecimalNumber, Rounded, Sign, decimal_text, floatformat, number_format, python_float_repr,
    python_float_str,
};
pub use number::{FileSizeStrings, FloatNumberFormats, NumberOptions};
pub use truncate::TruncationText;

static MARK_FILTER_SAFE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
pub(crate) static SAFEDATA: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static CONDITIONAL_ESCAPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SAFESTRING: PyOnceLock<Py<PyType>> = PyOnceLock::new();

impl Resolve for Filter {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
        failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        let resolved = match &self.left {
            crate::parse::TagElement::Variable(variable) => {
                variable.resolve(py, template, context, failures)
            }
            left => left.resolve(py, template, context, failures),
        };
        let mut left = match resolved {
            Err(PyRenderError::RenderError(RenderError::VariableDoesNotExist { .. }))
                if matches!(self.left, crate::parse::TagElement::Variable(_)) =>
            {
                None
            }
            result => result?,
        };
        if left.is_none()
            && failures == ResolveFailures::Raise
            && !context.string_if_invalid.is_empty()
            && let crate::parse::TagElement::Variable(variable) = &self.left
        {
            return Ok(Some(Content::Py(
                context.invalid_value(py, Some(template.content(variable.at)))?,
            )));
        }
        if left.is_none() && matches!(self.left, crate::parse::TagElement::Variable(_)) {
            // Django gives filters `None` for a missing variable when failures
            // are ignored, as in `{% if %}` and `{% for %}`; otherwise the
            // empty `string_if_invalid`.
            left = Some(match failures {
                ResolveFailures::IgnoreVariableDoesNotExist => {
                    Content::Py(py.None().into_bound(py))
                }
                ResolveFailures::Raise => "".as_content(),
            });
        }
        self.filter.resolve(left, py, template, context)
    }
}

impl ResolveFilter for FilterType {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        match self {
            Self::Add(filter) => filter.resolve(variable, py, template, context),
            Self::AddSlashes(filter) => filter.resolve(variable, py, template, context),
            Self::Capfirst(filter) => filter.resolve(variable, py, template, context),
            Self::Center(filter) => filter.resolve(variable, py, template, context),
            Self::Cut(filter) => filter.resolve(variable, py, template, context),
            Self::Default(filter) => filter.resolve(variable, py, template, context),
            Self::DefaultIfNone(filter) => filter.resolve(variable, py, template, context),
            Self::DivisibleBy(filter) => filter.resolve(variable, py, template, context),
            Self::Date(filter) => filter.resolve(variable, py, template, context),
            Self::Django(filter) => filter.resolve(variable, py, template, context),
            Self::Escape(filter) => filter.resolve(variable, py, template, context),
            Self::Escapejs(filter) => filter.resolve(variable, py, template, context),
            Self::External(filter) => filter.resolve(variable, py, template, context),
            Self::FloatFormat(filter) => filter.resolve(variable, py, template, context),
            Self::Tz(filter) => filter.resolve(variable, py, template, context),
            Self::L10n(filter) => filter.resolve(variable, py, template, context),
            Self::LanguageInfo(filter) => filter.resolve(variable, py, template, context),
            Self::Humanize(filter) => filter.resolve(variable, py, template, context),
            Self::First(filter) => filter.resolve(variable, py, template, context),
            Self::ForceEscape(filter) => filter.resolve(variable, py, template, context),
            Self::Join(filter) => filter.resolve(variable, py, template, context),
            Self::Justify(filter) => filter.resolve(variable, py, template, context),
            Self::Last(filter) => filter.resolve(variable, py, template, context),
            Self::Linebreaks(filter) => filter.resolve(variable, py, template, context),
            Self::Linebreaksbr(filter) => filter.resolve(variable, py, template, context),
            Self::Linenumbers(filter) => filter.resolve(variable, py, template, context),
            Self::Lower(filter) => filter.resolve(variable, py, template, context),
            Self::Length(filter) => filter.resolve(variable, py, template, context),
            Self::Safe(filter) => filter.resolve(variable, py, template, context),
            Self::Slugify(filter) => filter.resolve(variable, py, template, context),
            Self::Timesince(filter) => filter.resolve(variable, py, template, context),
            Self::Title(filter) => filter.resolve(variable, py, template, context),
            Self::TruncateChars(filter) => filter.resolve(variable, py, template, context),
            Self::TruncateCharsHtml(filter) => filter.resolve(variable, py, template, context),
            Self::Slice(filter) => filter.resolve(variable, py, template, context),
            Self::TruncateWords(filter) => filter.resolve(variable, py, template, context),
            Self::Pluralize(filter) => filter.resolve(variable, py, template, context),
            Self::GetDigit(filter) => filter.resolve(variable, py, template, context),
            Self::Phone2numeric(filter) => filter.resolve(variable, py, template, context),
            Self::Urlencode(filter) => filter.resolve(variable, py, template, context),
            Self::Iriencode(filter) => filter.resolve(variable, py, template, context),
            Self::MakeList(filter) => filter.resolve(variable, py, template, context),
            Self::Escapeseq(filter) => filter.resolve(variable, py, template, context),
            Self::Safeseq(filter) => filter.resolve(variable, py, template, context),
            Self::Random(filter) => filter.resolve(variable, py, template, context),
            Self::Pprint(filter) => filter.resolve(variable, py, template, context),
            Self::Filesizeformat(filter) => filter.resolve(variable, py, template, context),
            Self::UnorderedList(filter) => filter.resolve(variable, py, template, context),
            Self::Striptags(filter) => filter.resolve(variable, py, template, context),
            Self::Stringformat(filter) => filter.resolve(variable, py, template, context),
            Self::Dictsort(filter) => filter.resolve(variable, py, template, context),
            Self::Dictsortreversed(filter) => filter.resolve(variable, py, template, context),
            Self::JsonScript(filter) => filter.resolve(variable, py, template, context),
            Self::TruncateWordsHtml(filter) => filter.resolve(variable, py, template, context),
            Self::Urlize(filter) => filter.resolve(variable, py, template, context),
            Self::Urlizetrunc(filter) => filter.resolve(variable, py, template, context),
            Self::Upper(filter) => filter.resolve(variable, py, template, context),
            Self::Wordcount(filter) => filter.resolve(variable, py, template, context),
            Self::Wordwrap(filter) => filter.resolve(variable, py, template, context),
            Self::Yesno(filter) => filter.resolve(variable, py, template, context),
        }
    }
}

/// A filter argument Django uses as a `str`. Other values raise the error
/// Django's string method raises, built from the value's type name.
fn string_argument<'t>(
    argument: Content<'t, '_>,
    context: &Context,
    (py, at, template): (Python<'_>, dtl_lexer::types::At, TemplateString<'_>),
    error: impl FnOnce(&str) -> PyErr,
) -> Result<Cow<'t, str>, PyRenderError> {
    let name = match &argument {
        Content::String(_) => return Ok(argument.resolve_string(context)?.into_raw()),
        Content::Py(value) if value.is_instance_of::<PyString>() => {
            return Ok(argument.resolve_string(context)?.into_raw());
        }
        Content::Py(value) => value.get_type().name()?.to_string(),
        Content::Int(_) => "int".to_string(),
        Content::Float(_) => "float".to_string(),
        Content::Bool(_) => "bool".to_string(),
    };
    Err(error(&name).annotate(py, at, "here", template).into())
}

pub trait ResolveFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py>;
}

impl ResolveFilter for DefaultFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Django resolves a filter's arguments before calling it, so a
        // missing argument raises even when the value isn't replaced.
        let argument = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?;
        match variable {
            Some(left)
                if left
                    .to_bool()
                    .map_err(|err| err.annotate(py, self.at, "here", template))? =>
            {
                Ok(Some(left))
            }
            None | Some(_) => Ok(argument),
        }
    }
}

impl ResolveFilter for DefaultIfNoneFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // As for `default`, a missing argument always raises.
        let argument = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?;
        match variable {
            Some(Content::Py(ref value)) if value.is_none() => Ok(argument),
            Some(left) => Ok(Some(left)),
            None => Ok(Some("".as_content())),
        }
    }
}

/// The integer Django's `int(arg)` produces, for arguments that convert
/// unambiguously. Anything else returns `None` and is left to Django.
pub(crate) fn integer_argument(argument: &Content<'_, '_>) -> Option<i64> {
    match argument {
        Content::Int(n) => n.to_i64(),
        // `int()` truncates floats; infinities and NaN raise.
        Content::Float(n) => n.to_i64(),
        Content::Bool(b) => Some(i64::from(*b)),
        Content::String(text) => python_int(text.as_raw()),
        Content::Py(object) if object.is_exact_instance_of::<PyInt>() => object.extract().ok(),
        Content::Py(object) if object.is_exact_instance_of::<PyBool>() => {
            object.extract::<bool>().ok().map(i64::from)
        }
        Content::Py(object) if object.is_exact_instance_of::<PyFloat>() => {
            object.extract::<f64>().ok()?.to_i64()
        }
        Content::Py(object)
            if object.is_exact_instance_of::<PyString>()
                || is_exact_safe_string(object).unwrap_or(false) =>
        {
            python_int(object.cast::<PyString>().ok()?.to_str().ok()?)
        }
        Content::Py(_) => None,
    }
}

/// A string the truncation filters can shorten natively.
#[derive(Clone, Copy)]
enum Text<'a> {
    Plain(&'a str),
    /// Marked safe, so the result stays safe.
    Safe(&'a str),
}

impl<'a> Text<'a> {
    /// Plain and safe strings, which `str()` in Django's filters leaves as
    /// they are. Other values return `None` and are left to Django.
    fn from_variable(
        variable: Option<&'a Content<'_, '_>>,
        py: Python<'_>,
    ) -> PyResult<Option<Self>> {
        Ok(match variable {
            None => Some(Self::Plain("")),
            Some(Content::String(ContentString::HtmlSafe(content))) => Some(Self::Safe(content)),
            Some(Content::String(
                ContentString::String(content) | ContentString::HtmlUnsafe(content),
            )) => Some(Self::Plain(content)),
            Some(Content::Py(object)) => match object.cast::<PyString>() {
                Ok(string) if string.is_exact_instance_of::<PyString>() => {
                    string.to_str().ok().map(Self::Plain)
                }
                Ok(string)
                    if string.get_type().is(SAFESTRING.import(
                        py,
                        "django.utils.safestring",
                        "SafeString",
                    )?) =>
                {
                    string.to_str().ok().map(Self::Safe)
                }
                _ => None,
            },
            Some(Content::Float(_) | Content::Int(_) | Content::Bool(_)) => None,
        })
    }

    const fn as_str(self) -> &'a str {
        match self {
            Self::Plain(text) | Self::Safe(text) => text,
        }
    }

    /// The filter result for this text, safe only when the input was:
    /// Django's truncation filters are `is_safe`.
    fn into_content<'t, 'py>(self, result: String, context: &Context) -> Content<'t, 'py> {
        let result = Cow::Owned(result);
        Content::String(match self {
            Self::Safe(_) => ContentString::HtmlSafe(result),
            Self::Plain(_) if context.autoescape => ContentString::HtmlUnsafe(result),
            Self::Plain(_) => ContentString::String(result),
        })
    }
}

/// An `is_safe` `@stringfilter` result: safe only when the input was.
fn string_filter_result<'t, 'py>(
    result: String,
    safe: bool,
    context: &Context,
) -> Content<'t, 'py> {
    let result = Cow::Owned(result);
    Content::String(if safe {
        ContentString::HtmlSafe(result)
    } else if context.autoescape {
        ContentString::HtmlUnsafe(result)
    } else {
        ContentString::String(result)
    })
}

/// The text a `@stringfilter` sees and whether it is safe: `str()` keeps a
/// result safe only when it is `SafeData`. Digits, `True` and `False` need no
/// escaping, so they count as safe.
fn string_filter_text<'t>(
    variable: Option<Content<'t, '_>>,
    py: Python<'_>,
) -> PyResult<(Cow<'t, str>, bool)> {
    fn python_text<'t>(object: &Bound<'_, PyAny>) -> PyResult<(Cow<'t, str>, bool)> {
        let string = object.str()?;
        let safe = !string.is_exact_instance_of::<PyString>()
            && string.is_instance(SAFEDATA.import(
                object.py(),
                "django.utils.safestring",
                "SafeData",
            )?)?;
        Ok((Cow::Owned(string.to_str()?.to_string()), safe))
    }

    Ok(match variable {
        None => (Cow::Borrowed(""), true),
        Some(Content::String(ContentString::HtmlSafe(text))) => (text, true),
        Some(Content::String(ContentString::String(text) | ContentString::HtmlUnsafe(text))) => {
            (text, false)
        }
        Some(Content::Int(number)) => (Cow::Owned(number.to_string()), true),
        Some(Content::Bool(true)) => (Cow::Borrowed("True"), true),
        Some(Content::Bool(false)) => (Cow::Borrowed("False"), true),
        // Python's `str()` spelling, such as `1.0` and `1e+16`.
        Some(Content::Float(number)) => python_text(&PyFloat::new(py, number))?,
        Some(Content::Py(object)) => python_text(&object)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::gc::Gc;

    use super::*;
    use crate::filters::{AddSlashesFilter, DefaultFilter, LowerFilter, UpperFilter};
    use crate::parse::TagElement;
    use crate::render::Render;
    use crate::template::django_rusty_templates::{Engine, Template};
    use crate::types::{Argument, ArgumentType, Text};

    use pyo3::types::{PyDict, PyString};
    static MARK_SAFE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    fn mark_safe(py: Python<'_>, string: String) -> PyResult<Py<PyAny>> {
        let mark_safe = match MARK_SAFE.get(py) {
            Some(mark_safe) => mark_safe,
            None => {
                let py_mark_safe = py.import("django.utils.safestring")?;
                let py_mark_safe = py_mark_safe.getattr("mark_safe")?;
                MARK_SAFE.set(py, py_mark_safe.into()).unwrap();
                MARK_SAFE.get(py).unwrap()
            }
        };
        let safe_string = mark_safe.call1(py, (string,))?;
        Ok(safe_string)
    }

    use dtl_lexer::types::Variable;
    use std::collections::HashMap;

    #[test]
    fn test_render_filter() {
        Python::initialize();

        Python::attach(|py| {
            let name = PyString::new(py, "Lily").into_any();
            let context = HashMap::from([("name".to_string(), name.unbind())]);
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ name|default:'Bryony' }}");
            let variable = Variable::new((3, 4));
            let filter = Filter {
                at: (8, 7),
                all_at: (3, 12),
                left: TagElement::Variable(variable),
                filter: FilterType::Default(DefaultFilter::new(
                    Argument {
                        at: (16, 8),
                        argument_type: ArgumentType::Text(Text::new((17, 6))),
                    },
                    (8, 7),
                )),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "Lily");
        });
    }

    #[test]
    fn test_render_filter_slugify_happy_path() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|slugify }}".to_string();
            let context = PyDict::new(py);
            context.set_item("var", "hello world").unwrap();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "hello-world");
        });
    }

    #[test]
    fn test_render_filter_slugify_spaces_omitted() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|slugify }}".to_string();
            let context = PyDict::new(py);
            context.set_item("var", " hello world").unwrap();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "hello-world");
        });
    }

    #[test]
    fn test_render_filter_slugify_special_characters_omitted() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|slugify }}".to_string();
            let context = PyDict::new(py);
            context.set_item("var", "a&€%").unwrap();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "a");
        });
    }

    #[test]
    fn test_render_filter_slugify_multiple_spaces_inside_becomes_single() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|slugify }}".to_string();
            let context = PyDict::new(py);
            context.set_item("var", "a & b").unwrap();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "a-b");
        });
    }

    #[test]
    fn test_render_filter_slugify_integer() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|default:1|slugify }}".to_string();
            let context = PyDict::new(py);
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "1");
        });
    }

    #[test]
    fn test_render_filter_slugify_float() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|default:1.3|slugify }}".to_string();
            let context = PyDict::new(py);
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            // Django's `slugify` drops the decimal point.
            assert_eq!(result, "13");
        });
    }

    #[test]
    fn test_render_filter_slugify_rust_string() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|default:'hello world'|slugify }}".to_string();
            let context = PyDict::new(py);
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "hello-world");
        });
    }

    #[test]
    fn test_render_filter_slugify_safe_string() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|default:'hello world'|safe|slugify }}".to_string();
            let context = PyDict::new(py);
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "hello-world");
        });
    }

    #[test]
    fn test_render_filter_slugify_safe_string_from_rust_treated_as_py() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|slugify }}".to_string();
            let context = PyDict::new(py);
            let safe_string = mark_safe(py, "a &amp; b".to_string()).unwrap();
            context.set_item("var", safe_string).unwrap();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "a-amp-b");
        });
    }

    #[test]
    fn test_render_filter_slugify_non_existing_variable() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ not_there|slugify }}".to_string();
            let context = PyDict::new(py);
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "");
        });
    }

    #[test]
    fn test_render_filter_slugify_invalid() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|slugify:invalid }}".to_string();
            let error = Template::new_from_string(py, template_string, engine).unwrap_err();

            let error_string = error
                .value(py)
                .getattr("rusty_diagnostic")
                .unwrap()
                .to_string();
            assert!(error_string.contains("slugify filter does not take an argument"));
        });
    }

    #[test]
    fn test_render_filter_addslashes_single() {
        Python::initialize();

        Python::attach(|py| {
            let name = PyString::new(py, "'hello'").into_any();
            let context = HashMap::from([("quotes".to_string(), name.unbind())]);
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ quotes|addslashes }}");
            let variable = Variable::new((3, 6));
            let filter = Filter {
                at: (10, 10),
                all_at: (3, 17),
                left: TagElement::Variable(variable),
                filter: FilterType::AddSlashes(AddSlashesFilter),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, r"\'hello\'");
        });
    }

    #[test]
    fn test_render_filter_capfirst() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|capfirst }}".to_string();
            let context = PyDict::new(py);
            context.set_item("var", "hello world").unwrap();
            let template = Template::new_from_string(py, template_string, engine.clone()).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "Hello world");

            let context = PyDict::new(py);
            context.set_item("var", "").unwrap();
            let template_string = "{{ var|capfirst }}".to_string();
            let template = Template::new_from_string(py, template_string, engine.clone()).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "");

            let context = PyDict::new(py);
            context.set_item("bar", "").unwrap();
            let template_string = "{{ var|capfirst }}".to_string();
            let template = Template::new_from_string(py, template_string, engine.clone()).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "");

            let template_string = "{{ var|capfirst:invalid }}".to_string();
            let error = Template::new_from_string(py, template_string, engine).unwrap_err();

            let error_string = error
                .value(py)
                .getattr("rusty_diagnostic")
                .unwrap()
                .to_string();
            assert!(error_string.contains("capfirst filter does not take an argument"));
        });
    }

    #[test]
    fn test_render_filter_center() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|center:'11' }}".to_string();
            let context = PyDict::new(py);
            context.set_item("var", "hello").unwrap();
            let template = Template::new_from_string(py, template_string, engine.clone()).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "   hello   ");

            let context = PyDict::new(py);
            context.set_item("var", "django").unwrap();
            let template_string = "{{ var|center:'15' }}".to_string();
            let template = Template::new_from_string(py, template_string, engine.clone()).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "     django    ");

            let context = PyDict::new(py);
            context.set_item("var", "django").unwrap();
            let template_string = "{{ var|center:1 }}".to_string();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "django");
        });
    }

    #[test]
    fn test_render_filter_center_no_argument_return_err() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|center }}".to_string();
            let error = Template::new_from_string(py, template_string, engine).unwrap_err();

            let error_string = error
                .value(py)
                .getattr("rusty_diagnostic")
                .unwrap()
                .to_string();

            assert!(error_string.contains("Expected an argument"));
        });
    }

    #[test]
    fn test_render_filter_center_no_variable() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|center:'11' }}".to_string();
            let context = PyDict::new(py);
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            // Django centers `string_if_invalid`, "" by default (#376).
            assert_eq!(result, " ".repeat(11));
        });
    }

    #[test]
    fn test_render_filter_center_on_0() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ var|center:0 }}".to_string();
            let context = PyDict::new(py);
            context.set_item("var", "hello").unwrap();
            let template = Template::new_from_string(py, template_string, engine).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "hello");
        });
    }

    #[test]
    fn test_render_filter_default() {
        Python::initialize();

        Python::attach(|py| {
            let context = HashMap::new();
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ name|default:'Bryony' }}");
            let variable = Variable::new((3, 4));
            let filter = Filter {
                at: (8, 7),
                all_at: (3, 12),
                left: TagElement::Variable(variable),
                filter: FilterType::Default(DefaultFilter::new(
                    Argument {
                        at: (16, 8),
                        argument_type: ArgumentType::Text(Text::new((17, 6))),
                    },
                    (8, 7),
                )),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "Bryony");
        });
    }

    #[test]
    fn test_render_filter_default_integer() {
        Python::initialize();

        Python::attach(|py| {
            let context = HashMap::new();
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ count|default:12}}");
            let variable = Variable::new((3, 5));
            let filter = Filter {
                at: (9, 7),
                all_at: (3, 12),
                left: TagElement::Variable(variable),
                filter: FilterType::Default(DefaultFilter::new(
                    Argument {
                        at: (17, 2),
                        argument_type: ArgumentType::Int(12.into()),
                    },
                    (9, 7),
                )),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "12");
        });
    }

    #[test]
    fn test_render_filter_default_float() {
        Python::initialize();

        Python::attach(|py| {
            let context = HashMap::new();
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ count|default:3.5}}");
            let variable = Variable::new((3, 5));
            let filter = Filter {
                at: (9, 7),
                all_at: (3, 12),
                left: TagElement::Variable(variable),
                filter: FilterType::Default(DefaultFilter::new(
                    Argument {
                        at: (17, 3),
                        argument_type: ArgumentType::Float(3.5),
                    },
                    (9, 7),
                )),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "3.5");
        });
    }

    #[test]
    fn test_render_filter_default_variable() {
        Python::initialize();

        Python::attach(|py| {
            let me = PyString::new(py, "Lily").into_any();
            let context = HashMap::from([("me".to_string(), me.unbind())]);
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ name|default:me}}");
            let variable = Variable::new((3, 4));
            let filter = Filter {
                at: (8, 7),
                all_at: (3, 11),
                left: TagElement::Variable(variable),
                filter: FilterType::Default(DefaultFilter::new(
                    Argument {
                        at: (16, 2),
                        argument_type: ArgumentType::Variable(Variable::new((16, 2))),
                    },
                    (8, 7),
                )),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "Lily");
        });
    }

    #[test]
    fn test_render_filter_lower() {
        Python::initialize();

        Python::attach(|py| {
            let name = PyString::new(py, "Lily").into_any();
            let context = HashMap::from([("name".to_string(), name.unbind())]);
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ name|lower }}");
            let variable = Variable::new((3, 4));
            let filter = Filter {
                at: (8, 5),
                all_at: (3, 10),
                left: TagElement::Variable(variable),
                filter: FilterType::Lower(LowerFilter),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "lily");
        });
    }

    #[test]
    fn test_render_filter_lower_missing_left() {
        Python::initialize();

        Python::attach(|py| {
            let context = HashMap::new();
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ name|lower }}");
            let variable = Variable::new((3, 4));
            let filter = Filter {
                at: (8, 5),
                all_at: (3, 10),
                left: TagElement::Variable(variable),
                filter: FilterType::Lower(LowerFilter),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "");
        });
    }

    #[test]
    fn test_render_chained_filters() {
        Python::initialize();

        Python::attach(|py| {
            let context = HashMap::new();
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ name|default:'Bryony'|lower }}");
            let variable = Variable::new((3, 4));
            let default = Filter {
                at: (8, 7),
                all_at: (3, 12),
                left: TagElement::Variable(variable),
                filter: FilterType::Default(DefaultFilter::new(
                    Argument {
                        at: (16, 8),
                        argument_type: ArgumentType::Text(Text::new((17, 6))),
                    },
                    (8, 7),
                )),
            };
            let lower = Filter {
                at: (25, 5),
                all_at: (3, 27),
                left: TagElement::Filter(Box::new(default)),
                filter: FilterType::Lower(LowerFilter),
            };

            let rendered = lower.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "bryony");
        });
    }

    #[test]
    fn test_render_filter_upper() {
        Python::initialize();

        Python::attach(|py| {
            let name = PyString::new(py, "Foo").into_any();
            let context = HashMap::from([("name".to_string(), name.unbind())]);
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ name|upper }}");
            let variable = Variable::new((3, 4));
            let filter = Filter {
                at: (8, 5),
                all_at: (3, 10),
                left: TagElement::Variable(variable),
                filter: FilterType::Upper(UpperFilter),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "FOO");
        });
    }

    #[test]
    fn test_render_filter_upper_missing_left() {
        Python::initialize();

        Python::attach(|py| {
            let context = HashMap::new();
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ name|upper }}");
            let variable = Variable::new((3, 4));
            let filter = Filter {
                at: (8, 5),
                all_at: (3, 10),
                left: TagElement::Variable(variable),
                filter: FilterType::Upper(UpperFilter),
            };

            let rendered = filter.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "");
        });
    }
}
