use std::borrow::Cow;

use html_escape::encode_quoted_attribute_to_string;
use num_traits::{ToPrimitive, Zero};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyDateTime, PyFloat, PyInt, PyList, PyString, PyTuple, PyType};

use crate::error::{AnnotatePyErr, PyRenderError, RenderError};
use crate::filters::{
    AddFilter, AddSlashesFilter, CapfirstFilter, CenterFilter, CutFilter, DateFilter,
    DefaultFilter, DefaultIfNoneFilter, DivisibleByFilter, DjangoFilter, EscapeFilter,
    EscapejsFilter, ExternalFilter, FilterType, FloatFormatFilter, ForceEscapeFilter, LastFilter,
    LengthFilter, LowerFilter, SafeFilter, SlugifyFilter, TitleFilter, TruncateCharsFilter,
    UpperFilter, WordcountFilter, WordwrapFilter, YesnoFilter,
};
use crate::parse::Filter;
use crate::render::common::gettext;
use crate::render::dateformat::{Formatted, Formatter, template_localtime};
use crate::render::types::{AsBorrowedContent, Content, ContentString, Context, IntoOwnedContent};
use crate::render::{Resolve, ResolveFailures, ResolveResult};
use dtl_lexer::types::TemplateString;
use unicode_normalization::UnicodeNormalization;

static MARK_FILTER_SAFE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

static SAFEDATA: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static DATE_FILTER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static TIME_FILTER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static PGETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static COMBINING: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SAFESTRING: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static GET_FORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DECIMAL: PyOnceLock<Py<PyType>> = PyOnceLock::new();

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
        if left.is_none()
            && matches!(failures, ResolveFailures::IgnoreVariableDoesNotExist)
            && matches!(self.filter, FilterType::DefaultIfNone(_))
        {
            left = Some(Content::Py(py.None().into_bound(py)));
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
            Self::ForceEscape(filter) => filter.resolve(variable, py, template, context),
            Self::Last(filter) => filter.resolve(variable, py, template, context),
            Self::Lower(filter) => filter.resolve(variable, py, template, context),
            Self::Length(filter) => filter.resolve(variable, py, template, context),
            Self::Safe(filter) => filter.resolve(variable, py, template, context),
            Self::Slugify(filter) => filter.resolve(variable, py, template, context),
            Self::Title(filter) => filter.resolve(variable, py, template, context),
            Self::TruncateChars(filter) => filter.resolve(variable, py, template, context),
            Self::Upper(filter) => filter.resolve(variable, py, template, context),
            Self::Wordcount(filter) => filter.resolve(variable, py, template, context),
            Self::Wordwrap(filter) => filter.resolve(variable, py, template, context),
            Self::Yesno(filter) => filter.resolve(variable, py, template, context),
        }
    }
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

impl ResolveFilter for AddFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(variable) = variable else {
            return Ok(None);
        };
        let right = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?
            .expect("missing argument in context should already have raised");
        Ok(match (variable.to_bigint(), right.to_bigint()) {
            (Some(variable), Some(right)) => Some(Content::Int(variable + right)),
            _ => {
                let variable = variable.to_py(py);
                let right = right.to_py(py);
                variable.add(right).ok().map(Content::Py)
            }
        })
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
        let Some(content) = variable else {
            return Ok(Some("".as_content()));
        };
        let content = content.resolve_string(context)?;
        let length = content.as_raw().chars().count();
        let arg = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?
            .expect("missing argument in context should already have raised");

        if context.django_series(py)? >= (6, 0) {
            let centered = py
                .import("django.template.defaultfilters")?
                .getattr("center")?
                .call1((content.as_raw(), arg.to_py(py)))?
                .extract::<String>()?;
            return Ok(Some(content.map_content(|_| Cow::Owned(centered))));
        }

        let size = arg.resolve_usize(self.argument.at)?;

        if size <= length {
            return Ok(Some(Content::String(content)));
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
        let Some(variable) = variable else {
            return Ok(Some("".as_content()));
        };

        let arg = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?
            .expect("missing argument in context should already have raised")
            .resolve_string_strict(context, self.argument.at.into())?
            .into_raw();

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

impl ResolveFilter for DefaultFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        match variable {
            Some(left)
                if left
                    .to_bool()
                    .map_err(|err| err.annotate(py, self.at, "here", template))? =>
            {
                Ok(Some(left))
            }
            None | Some(_) => self
                .argument
                .resolve(py, template, context, ResolveFailures::Raise),
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
        match variable {
            Some(Content::Py(ref value)) if value.is_none() => {
                self.argument
                    .resolve(py, template, context, ResolveFailures::Raise)
            }
            Some(left) => Ok(Some(left)),
            None => Ok(Some("".as_content())),
        }
    }
}

impl ResolveFilter for DateFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(value) = variable else {
            return Ok(Some("".as_content()));
        };
        let format = match &self.argument {
            Some(arg) => Some(
                arg.resolve(py, template, context, ResolveFailures::Raise)?
                    .expect("missing argument in context should already have raised"),
            ),
            None => None,
        };
        let date = DATE_FILTER.import(py, "django.template.defaultfilters", "date")?;
        let formatted = format_date(value, format, Formatter::Date, date, context)
            .map_err(|error| error.annotate(py, self.at, "here", template))?;
        Ok(Some(formatted))
    }
}

/// Django's `date` and `time` filters with `expects_localtime=True` applied.
/// Formats natively where [`DateCache`](super::dateformat::DateCache) can
/// reproduce Django exactly and otherwise calls `django_filter`.
fn format_date<'t, 'py>(
    value: Content<'t, 'py>,
    format: Option<Content<'t, 'py>>,
    formatter: Formatter,
    django_filter: &Bound<'py, PyAny>,
    context: &mut Context,
) -> PyResult<Content<'t, 'py>> {
    let py = django_filter.py();
    let value = value.to_py(py);
    if value.is_none() {
        return Ok("".as_content());
    }
    let value = template_localtime(value, context.use_tz, &mut context.date_cache)?;
    if let Some(format_name) = format_name(format.as_ref(), formatter)?
        && let Some(formatted) = context.date_cache.format(&value, &format_name, formatter)?
    {
        return Ok(match formatted {
            Formatted::Native(formatted) => Content::String(match context.autoescape {
                false => ContentString::String(Cow::Owned(formatted)),
                true => ContentString::HtmlUnsafe(Cow::Owned(formatted)),
            }),
            Formatted::Django(formatted) => Content::Py(formatted),
        });
    }
    let formatted = match format {
        Some(format) => django_filter.call1((value, format.to_py(py)))?,
        None => django_filter.call1((value,))?,
    };
    Ok(Content::Py(formatted))
}

/// The name passed to `get_format(format or "DATE_FORMAT")`. `None` for
/// argument types that are left to Django.
fn format_name<'a>(
    format: Option<&'a Content<'_, '_>>,
    formatter: Formatter,
) -> PyResult<Option<Cow<'a, str>>> {
    let name = match format {
        None => Cow::Borrowed(""),
        Some(Content::String(format)) => Cow::Borrowed(format.as_raw().as_ref()),
        Some(Content::Py(format)) if format.is_none() => Cow::Borrowed(""),
        Some(Content::Py(format)) => match format.cast_exact::<PyString>() {
            Ok(format) => format.to_cow()?,
            Err(_) => return Ok(None),
        },
        Some(Content::Float(_) | Content::Int(_) | Content::Bool(_)) => return Ok(None),
    };
    Ok(Some(match name.is_empty() {
        true => Cow::Borrowed(formatter.default_format()),
        false => name,
    }))
}

impl ResolveFilter for DivisibleByFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(variable) = variable else {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "invalid literal for int() with base 10: 'None'",
            )
            .annotate(py, self.at, "here", template)
            .into());
        };

        if let Content::Py(ref obj) = variable
            && obj.is_none()
        {
            return Err(pyo3::exceptions::PyTypeError::new_err(
                    "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'",
                )
                .annotate(py, self.at, "here", template)
                .into());
        }

        let right = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?
            .expect("missing argument in context should already have raised");

        let Some(right_val) = right.to_bigint() else {
            return Err(
                pyo3::exceptions::PyTypeError::new_err("Integer argument expected")
                    .annotate(py, self.argument.at, "here", template)
                    .into(),
            );
        };

        if right_val.is_zero() {
            return Err(pyo3::exceptions::PyZeroDivisionError::new_err(
                "Invalid divisibility check: cannot divide by zero",
            )
            .annotate(py, self.argument.at, "here", template)
            .into());
        }

        let Some(left_val) = variable.to_bigint() else {
            let value_str = variable.render(context)?;
            let msg = format!("invalid literal for int() with base 10: '{value_str}'");

            return Err(pyo3::exceptions::PyValueError::new_err(msg)
                .annotate(py, self.at, "here", template)
                .into());
        };

        Ok(Some(Content::Bool((left_val % right_val).is_zero())))
    }
}

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
                    Content::Float(n) => Cow::Owned(n.to_string()),
                    Content::Py(object) => {
                        let escaped = py
                            .import("django.utils.html")?
                            .getattr("conditional_escape")?
                            .call1((object,))?;
                        Cow::Owned(escaped.extract::<String>()?)
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

impl ResolveFilter for ExternalFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let arg = match &self.argument {
            Some(arg) => arg.resolve(py, template, context, ResolveFailures::Raise)?,
            None => None,
        };
        let filter = self.filter.bind(py);
        let value = match arg {
            Some(arg) => filter.call1((variable, arg))?,
            None => filter.call1((variable,))?,
        };
        Ok(Some(Content::Py(value)))
    }
}

/// A filter argument resolved once, so a native port and Django's filter see
/// the same value.
enum ResolvedArgument<'t, 'py> {
    /// The filter was used without an argument.
    Absent,
    /// The argument's value, `None` when it resolved to nothing. Django's
    /// filter then receives `None`.
    Present(Option<Content<'t, 'py>>),
}

impl<'t, 'py> ResolvedArgument<'t, 'py> {
    fn value(&self) -> Option<&Content<'t, 'py>> {
        match self {
            Self::Absent => None,
            Self::Present(value) => value.as_ref(),
        }
    }

    fn into_value(self) -> Option<Content<'t, 'py>> {
        match self {
            Self::Absent => None,
            Self::Present(value) => value,
        }
    }
}

impl DjangoFilter {
    fn resolve_argument<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<ResolvedArgument<'t, 'py>, PyRenderError> {
        Ok(match &self.argument {
            None => ResolvedArgument::Absent,
            Some(argument) => ResolvedArgument::Present(argument.resolve(
                py,
                template,
                context,
                ResolveFailures::Raise,
            )?),
        })
    }

    fn call<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        argument: ResolvedArgument<'t, 'py>,
        py: Python<'py>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let filter = self.filter.bind(py);
        let value = variable.unwrap_or_else(|| "".as_content());
        if filter.is(TIME_FILTER.import(py, "django.template.defaultfilters", "time")?) {
            return Ok(Some(format_date(
                value,
                argument.into_value(),
                Formatter::Time,
                filter,
                context,
            )?));
        }
        let mut value = value.to_py(py);
        if value.is_instance_of::<PyDateTime>()
            && filter
                .getattr_opt("expects_localtime")?
                .is_some_and(|flag| flag.is_truthy().unwrap_or(false))
        {
            value = template_localtime(value, context.use_tz, &mut context.date_cache)?;
        }
        let safe_type = SAFEDATA.import(py, "django.utils.safestring", "SafeData")?;
        let preserve_safe = filter
            .getattr("is_safe")
            .is_ok_and(|flag| flag.is_truthy().unwrap_or(false))
            && value.is_instance(safe_type)?;
        let mut args = vec![value];
        match argument {
            ResolvedArgument::Absent => {}
            ResolvedArgument::Present(value) => {
                args.push(value.map_or_else(|| py.None().into_bound(py), |value| value.to_py(py)));
            }
        }
        let kwargs = if filter
            .getattr("needs_autoescape")
            .is_ok_and(|flag| flag.is_truthy().unwrap_or(false))
        {
            let kwargs = pyo3::types::PyDict::new(py);
            kwargs.set_item("autoescape", context.autoescape)?;
            Some(kwargs)
        } else {
            None
        };
        let result = filter.call(pyo3::types::PyTuple::new(py, args)?, kwargs.as_ref())?;
        let result = if preserve_safe {
            MARK_FILTER_SAFE
                .import(py, "django.utils.safestring", "mark_safe")?
                .call1((result,))?
        } else {
            result
        };
        Ok(Some(Content::Py(result)))
    }
}

impl ResolveFilter for DjangoFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.resolve_argument(py, template, context)?;
        self.call(variable, argument, py, context)
    }
}

/// The integer Django's `int(arg)` produces, for arguments that convert
/// unambiguously. Anything else returns `None` and is left to Django.
fn integer_argument(argument: &Content<'_, '_>) -> Option<i64> {
    fn parse(text: &str) -> Option<i64> {
        let digits = text.strip_prefix('-').unwrap_or(text);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    }
    match argument {
        Content::Int(n) => n.to_i64(),
        Content::String(text) => parse(text.as_raw()),
        Content::Py(object) if object.is_exact_instance_of::<PyInt>() => object.extract().ok(),
        Content::Py(object) if object.is_exact_instance_of::<PyString>() => {
            parse(object.cast::<PyString>().ok()?.to_str().ok()?)
        }
        Content::Py(_) | Content::Float(_) | Content::Bool(_) => None,
    }
}

/// The number formatting `django.utils.formats.number_format()` reads for
/// the active language.
#[derive(Clone, Debug)]
pub struct FloatNumberFormat {
    decimal_separator: String,
    thousand_separator: String,
    /// Django's digit group sizes, read from the right; empty when
    /// `USE_THOUSAND_SEPARATOR` is off or `NUMBER_GROUPING` is `0`.
    grouping: Vec<i64>,
}

impl FloatNumberFormat {
    /// Returns `None` for settings Django would reject or treat specially,
    /// leaving those to Django.
    fn new(py: Python<'_>, language: &Bound<'_, PyString>) -> PyResult<Option<Self>> {
        let get_format = GET_FORMAT.import(py, "django.utils.formats", "get_format")?;
        let format = |name| get_format.call1((name, language, true));
        let (Ok(decimal_separator), Ok(thousand_separator)) = (
            format("DECIMAL_SEPARATOR")?.extract(),
            format("THOUSAND_SEPARATOR")?.extract(),
        ) else {
            return Ok(None);
        };
        let grouping = format("NUMBER_GROUPING")?;
        let use_grouping = SETTINGS
            .import(py, "django.conf", "settings")?
            .getattr("USE_THOUSAND_SEPARATOR")?
            .is_truthy()?
            && grouping.ne(0)?;
        let grouping = if !use_grouping {
            Vec::new()
        } else if grouping.is_exact_instance_of::<PyInt>() {
            match grouping.extract() {
                Ok(size) => vec![size, 0],
                Err(_) => return Ok(None),
            }
        } else if grouping.is_instance_of::<PyList>() || grouping.is_instance_of::<PyTuple>() {
            // An empty sequence makes Django raise IndexError.
            match grouping.extract::<Vec<i64>>() {
                Ok(sizes) if !sizes.is_empty() => sizes,
                _ => return Ok(None),
            }
        } else {
            return Ok(None);
        };
        Ok(Some(Self {
            decimal_separator,
            thousand_separator,
            grouping,
        }))
    }

    /// `django.utils.numberformat.format()` for a number already rounded to
    /// its decimal places.
    fn format(&self, number: &Rounded) -> String {
        let Rounded {
            sign,
            integer,
            fraction,
        } = number;
        let mut output = String::with_capacity(integer.len() * 2 + fraction.len() + 2);
        output.push_str(sign.prefix());
        if self.grouping.is_empty() {
            output.push_str(integer);
        } else {
            // Port of Django's loop, which builds the grouped digits reversed.
            let mut sizes = self.grouping.iter().copied();
            let mut size = sizes.next().unwrap_or_default();
            let mut count = 0;
            let mut reversed = String::with_capacity(integer.len() * 2);
            for digit in integer.chars().rev() {
                if count != 0 && count == size {
                    if let Some(next) = sizes.next()
                        && next != 0
                    {
                        size = next;
                    }
                    reversed.extend(self.thousand_separator.chars().rev());
                    count = 0;
                }
                reversed.push(digit);
                count += 1;
            }
            output.extend(reversed.chars().rev());
        }
        if !fraction.is_empty() {
            output.push_str(&self.decimal_separator);
            output.push_str(fraction);
        }
        output
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sign {
    Plus,
    Minus,
}

impl Sign {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Plus => "",
            Self::Minus => "-",
        }
    }
}

/// A number rounded to its decimal places, ready to localize.
struct Rounded {
    sign: Sign,
    integer: String,
    fraction: String,
}

/// A finite decimal number, `digits × 10^exponent`, as `Decimal(str(value))`
/// sees an `int`, `float` or `Decimal`.
struct DecimalNumber {
    sign: Sign,
    digits: Vec<u8>,
    exponent: i64,
}

impl DecimalNumber {
    /// Parses Python's `str()` of a number. Infinities and NaNs return `None`.
    fn parse(text: &str) -> Option<Self> {
        let (sign, text) = match text.strip_prefix('-') {
            Some(text) => (Sign::Minus, text),
            None => (Sign::Plus, text),
        };
        let (mantissa, exponent) = match text.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
            None => (text, 0),
        };
        let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let digits: Vec<u8> = integer
            .bytes()
            .chain(fraction.bytes())
            .map(|byte| byte.is_ascii_digit().then(|| byte - b'0'))
            .collect::<Option<_>>()?;
        if digits.is_empty() {
            return None;
        }
        Some(Self {
            sign,
            digits,
            exponent: exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?,
        })
    }

    /// Rounds half up to `places` decimal places, like `Decimal.quantize()`
    /// with `ROUND_HALF_UP`, which rounds the magnitude half away from zero.
    fn round(&self, places: usize) -> Option<Rounded> {
        let shift = self.exponent.checked_add(i64::try_from(places).ok()?)?;
        let mut digits = self.digits.clone();
        if let Ok(zeros) = usize::try_from(shift) {
            digits.resize(digits.len() + zeros, 0);
        } else {
            let dropped = usize::try_from(shift.unsigned_abs()).ok()?;
            let kept = digits.len().saturating_sub(dropped);
            let round_up = dropped <= digits.len() && digits[kept] >= 5;
            digits.truncate(kept);
            if round_up {
                match digits.iter().rposition(|&digit| digit != 9) {
                    Some(index) => {
                        digits[index] += 1;
                        digits[index + 1..].fill(0);
                    }
                    None => {
                        digits.fill(0);
                        digits.insert(0, 1);
                    }
                }
            }
        }
        // Like Django, a number that rounds to zero loses its sign: -0.01 is "0.0".
        let sign = if digits.iter().all(|&digit| digit == 0) {
            Sign::Plus
        } else {
            self.sign
        };
        let leading_zeros = digits.iter().take_while(|&&digit| digit == 0).count();
        let digits = &digits[leading_zeros..];
        let split = digits.len().saturating_sub(places);
        let to_text = |digits: &[u8]| -> String {
            digits
                .iter()
                .map(|&digit| char::from(b'0' + digit))
                .collect()
        };
        let integer = match split {
            0 => "0".to_string(),
            _ => to_text(&digits[..split]),
        };
        let fraction = format!("{:0>places$}", to_text(&digits[split..]));
        Some(Rounded {
            sign,
            integer,
            fraction,
        })
    }
}

/// Django's limit on digits plus exponent, beyond which `floatformat`
/// returns the input unchanged. Larger precisions are also left to Django.
const FLOATFORMAT_MAX_DIGITS: usize = 200;

/// `floatformat` for exact `int`, `float` and `Decimal` values and an
/// integer precision. `None` defers to Django.
fn floatformat(
    value: &Content<'_, '_>,
    precision: i64,
    py: Python<'_>,
    context: &mut Context,
) -> PyResult<Option<String>> {
    let text = match value {
        Content::Int(n) => n.to_string(),
        Content::Float(n) => PyFloat::new(py, *n).str()?.to_string(),
        Content::Py(object)
            if object.is_exact_instance_of::<PyInt>()
                || object.is_exact_instance_of::<PyFloat>()
                || object
                    .get_type()
                    .is(DECIMAL.import(py, "decimal", "Decimal")?) =>
        {
            match object.str() {
                Ok(text) => text.to_string(),
                Err(_) => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    let Some(number) = DecimalNumber::parse(&text) else {
        return Ok(None);
    };
    let places = usize::try_from(precision.unsigned_abs()).unwrap_or(usize::MAX);
    if places > FLOATFORMAT_MAX_DIGITS
        || usize::try_from(number.exponent.unsigned_abs())
            .ok()
            .and_then(|exponent| exponent.checked_add(number.digits.len()))
            .is_none_or(|size| size > FLOATFORMAT_MAX_DIGITS)
    {
        return Ok(None);
    }
    // Whole numbers drop their decimal places unless the precision is positive.
    let fraction_digits = usize::try_from(-number.exponent).unwrap_or(0);
    let whole = number
        .digits
        .iter()
        .rev()
        .take(fraction_digits)
        .all(|&digit| digit == 0);
    let places = if whole && precision <= 0 { 0 } else { places };
    let Some(rounded) = number.round(places) else {
        return Ok(None);
    };
    let Some(Some(format)) = context
        .float_number_format
        .get_or_try_init(py, |language| FloatNumberFormat::new(py, language))?
    else {
        return Ok(None);
    };
    Ok(Some(format.format(&rounded)))
}

impl ResolveFilter for FloatFormatFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        let precision = match &argument {
            ResolvedArgument::Absent => Some(-1),
            ResolvedArgument::Present(value) => value.as_ref().and_then(integer_argument),
        };
        if let Some(precision) = precision
            && let Some(value) = &variable
            && let Some(formatted) = floatformat(value, precision, py, context)?
        {
            return Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
                formatted,
            )))));
        }
        self.django.call(variable, argument, py, context)
    }
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
                    Content::Float(n) => Cow::Owned(n.to_string()),
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

/// Convert spaces or repeated dashes to single dashes.
///
/// Remove characters that aren't ascii alphanumerics, underscores, or hyphens.
/// Convert to lowercase. Also strip leading and trailing whitespace, dashes, and underscores.
///
/// See <https://github.com/django/django/blob/stable/5.2.x/django/utils/text.py#L453>
pub fn slugify(content: Cow<str>) -> Cow<str> {
    let mut slug = String::with_capacity(content.len());

    // treat start as if preceded by dash to strip leading dashes
    let mut prev_dash = true;
    for c in content.as_ref().nfkd().filter(char::is_ascii) {
        if c.is_ascii_alphanumeric() || c == '_' {
            slug.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if (c.is_whitespace() || c == '-') && !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.truncate(slug.trim_end_matches(['-', '_']).len());
    Cow::Owned(slug)
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
                // Int and Float requires no slugify, we only need to turn it into a string.
                Content::Int(content) => content.to_string().into_content(),
                Content::Float(content) => content.to_string().into_content(),
                Content::String(content) => content.map_content(slugify),
                Content::Bool(true) => "true".as_content(),
                Content::Bool(false) => "false".as_content(),
            },
            None => "".as_content(),
        };
        Ok(Some(content))
    }
}

/// Django's translated truncation text, `%(truncated_text)s…` in English,
/// split around the placeholder.
#[derive(Clone, Debug)]
pub struct TruncationText {
    before: String,
    after: String,
    /// The number of characters Django counts: combining characters are free.
    width: usize,
}

impl TruncationText {
    /// Returns `None` when the translation is not a plain
    /// `%(truncated_text)s` template, leaving its %-formatting to Django.
    fn new(py: Python<'_>) -> PyResult<Option<Self>> {
        let text: String = PGETTEXT
            .import(py, "django.utils.translation", "pgettext")?
            .call1((
                "String to return when truncating text",
                "%(truncated_text)s…",
            ))?
            .extract()?;
        if text.matches('%').count() != 1 {
            return Ok(None);
        }
        let Some((before, after)) = text.split_once("%(truncated_text)s") else {
            return Ok(None);
        };
        let combining = COMBINING.import(py, "unicodedata", "combining")?;
        let mut width = 0;
        for character in before.chars().chain(after.chars()) {
            // Ask Python so the result follows its Unicode database.
            if character.is_ascii() || !combining.call1((character,))?.is_truthy()? {
                width += 1;
            }
        }
        Ok(Some(Self {
            before: before.to_string(),
            after: after.to_string(),
            width,
        }))
    }
}

/// A string `truncatechars` can shorten natively.
#[derive(Clone, Copy)]
enum Text<'a> {
    Plain(&'a str),
    /// Marked safe, so the result stays safe.
    Safe(&'a str),
}

impl<'a> Text<'a> {
    const fn as_str(self) -> &'a str {
        match self {
            Self::Plain(text) | Self::Safe(text) => text,
        }
    }
}

/// `Truncator(text).chars(length)` for ASCII text, where every character
/// counts once and NFC normalization changes nothing. `None` defers to Django.
fn truncate_ascii_chars(
    text: &str,
    length: i64,
    py: Python<'_>,
    context: &mut Context,
) -> PyResult<Option<String>> {
    if !text.is_ascii() {
        return Ok(None);
    }
    let Ok(length @ 1..) = usize::try_from(length) else {
        return Ok(Some(String::new()));
    };
    if text.len() <= length {
        return Ok(Some(text.to_string()));
    }
    let Some(Some(truncation)) = context
        .truncation_text
        .get_or_try_init(py, |_| TruncationText::new(py))?
    else {
        return Ok(None);
    };
    let end = length.saturating_sub(truncation.width);
    Ok(Some(format!(
        "{}{}{}",
        truncation.before,
        &text[..end],
        truncation.after
    )))
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
        // Plain and safe strings, which `str()` in Django's filter leaves as they are.
        let text = match &variable {
            None => Some(Text::Plain("")),
            Some(Content::String(ContentString::HtmlSafe(content))) => Some(Text::Safe(content)),
            Some(Content::String(
                ContentString::String(content) | ContentString::HtmlUnsafe(content),
            )) => Some(Text::Plain(content)),
            Some(Content::Py(object)) => match object.cast::<PyString>() {
                Ok(string) if string.is_exact_instance_of::<PyString>() => {
                    string.to_str().ok().map(Text::Plain)
                }
                Ok(string)
                    if string.get_type().is(SAFESTRING.import(
                        py,
                        "django.utils.safestring",
                        "SafeString",
                    )?) =>
                {
                    string.to_str().ok().map(Text::Safe)
                }
                _ => None,
            },
            Some(Content::Float(_) | Content::Int(_) | Content::Bool(_)) => None,
        };
        if let Some(length) = length
            && let Some(text) = text
            && let Some(truncated) = truncate_ascii_chars(text.as_str(), length, py, context)?
        {
            let truncated = Cow::Owned(truncated);
            return Ok(Some(Content::String(match text {
                Text::Safe(_) => ContentString::HtmlSafe(truncated),
                Text::Plain(_) if context.autoescape => ContentString::HtmlUnsafe(truncated),
                Text::Plain(_) => ContentString::String(truncated),
            })));
        }
        self.django.call(variable, argument, py, context)
    }
}

impl ResolveFilter for TitleFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        _py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(content) = variable else {
            return Ok(Some("".as_content()));
        };
        let content_string = content.resolve_string(context)?;

        Ok(Some(content_string.map_content(|content| {
            let mut result = String::with_capacity(content.len());
            let mut prev = None;
            let mut prev_letter_was_lowercased = false;

            for ch in content.chars() {
                if ch.is_alphabetic() {
                    // Django's special cases to trigger lowercase:
                    // 1. After apostrophe that follows a lowercased letter
                    // 2. After a digit
                    let should_lowercase = match prev {
                        Some('\'') if prev_letter_was_lowercased => true,
                        Some(c) if c.is_ascii_digit() | c.is_alphabetic() => true,
                        _ => false,
                    };

                    if should_lowercase {
                        result.extend(ch.to_lowercase());
                    } else {
                        result.extend(ch.to_uppercase());
                    }
                    prev_letter_was_lowercased = should_lowercase;
                } else {
                    result.push(ch);
                }
                prev = Some(ch);
            }
            Cow::Owned(result)
        })))
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
    let mut result = String::with_capacity(text.len());

    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            result.push('\n');
        }

        if line.is_empty() {
            continue;
        }

        let leading_whitespace = line.chars().take_while(|c| c.is_whitespace()).count();
        let (indent, trimmed_line) = line.split_at(leading_whitespace);

        let mut words = trimmed_line.split_whitespace();

        let Some(first_word) = words.next() else {
            // Line contains only whitespace - preserve it
            result.push_str(line);
            continue;
        };

        result.push_str(indent);
        result.push_str(first_word);

        let mut current_len = leading_whitespace + first_word.len();
        for word in words {
            let word_len = word.len();

            if current_len + word_len < width {
                result.push(' ');
                current_len += 1 + word_len;
            } else {
                result.push('\n');
                current_len = word_len;
            }
            result.push_str(word);
        }
    }

    result
}

impl ResolveFilter for WordwrapFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let Some(content) = variable else {
            return Ok(Some("".as_content()));
        };

        let text = content.resolve_string(context)?;

        let arg = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?
            .expect("missing argument in context should already have raised");

        // Check for negative values before converting to usize
        if let Some(bigint) = arg.to_bigint()
            && let Some(n) = bigint.to_isize()
            && n <= 0
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
            Err(e) => return Err(e),
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
                arg_content
                    .resolve_string_strict(context, arg.at.into())?
                    .into_raw()
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

            assert_eq!(result, "1.3");
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

            assert_eq!(result, "");
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
