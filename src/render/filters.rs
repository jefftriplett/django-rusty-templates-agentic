use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use html_escape::encode_quoted_attribute_to_string;
use num_traits::{ToPrimitive, Zero};
use pyo3::exceptions::{PyAttributeError, PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{
    PyBool, PyDateTime, PyDict, PyFloat, PyInt, PyList, PyRange, PySlice, PyString, PyTuple, PyType,
};

use crate::error::{AnnotatePyErr, PyRenderError, RenderError};
use crate::filters::{
    AddFilter, AddSlashesFilter, CapfirstFilter, CenterFilter, CutFilter, DateFilter,
    DefaultFilter, DefaultIfNoneFilter, DictsortFilter, DictsortreversedFilter, DivisibleByFilter,
    DjangoFilter, EscapeFilter, EscapejsFilter, EscapeseqFilter, ExternalFilter,
    FilesizeformatFilter, FilterType, FirstFilter, FloatFormatFilter, ForceEscapeFilter,
    GetDigitFilter, IriencodeFilter, JoinFilter, JsonScriptFilter, Justify, JustifyFilter,
    LastFilter, LengthFilter, LinebreaksFilter, LinebreaksbrFilter, LinenumbersFilter, LowerFilter,
    MakeListFilter, Phone2numericFilter, PluralizeFilter, PprintFilter, RandomFilter, SafeFilter,
    SafeseqFilter, SliceArgument, SliceFilter, SlugifyFilter, StringformatFilter, StriptagsFilter,
    TimesinceFilter, TitleFilter, TruncateCharsFilter, TruncateCharsHtmlFilter,
    TruncateWordsFilter, TruncateWordsHtmlFilter, UnorderedListFilter, UpperFilter,
    UrlencodeFilter, UrlizeFilter, UrlizetruncFilter, WordcountFilter, WordwrapFilter, YesnoFilter,
    python_int,
};
use crate::parse::Filter;
use crate::render::common::gettext;
use crate::render::dateformat::{Formatted, Formatter, template_localtime};
use crate::render::l10n::Grouping;
use crate::render::timesince::{TimeSince, TimeStrings, timesince};
use crate::render::types::{
    AsBorrowedContent, Content, ContentString, Context, IntoOwnedContent, append_escaped,
    append_python_string,
};
use crate::render::{Resolve, ResolveFailures, ResolveResult};
use dtl_lexer::types::TemplateString;
use unicode_normalization::UnicodeNormalization;

static MARK_FILTER_SAFE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

static SAFEDATA: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static CONDITIONAL_ESCAPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
pub static DATE_FILTER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static TIME_FILTER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static PGETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static COMBINING: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static TRUNCATOR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static NORMALIZE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
/// `unicodedata.combining(character) != 0` for characters already asked.
/// Python's Unicode database doesn't change while it runs.
static COMBINING_CHARACTERS: std::sync::Mutex<std::collections::BTreeMap<char, bool>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
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
            && matches!(
                self.filter,
                FilterType::DefaultIfNone(_) | FilterType::Django(_) | FilterType::External(_)
            )
        {
            // Django gives filters `None` for a missing variable when failures
            // are ignored, as in `{% if %}` and `{% for %}`; otherwise the
            // empty `string_if_invalid`.
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
            None => arg.resolve_usize(self.argument.at)?,
        };

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
    format_localtime(value, format, formatter, django_filter, context)
}

/// [`format_date`] for a value already in the time zone to format it in.
pub fn format_localtime<'t, 'py>(
    value: Bound<'py, PyAny>,
    format: Option<Content<'t, 'py>>,
    formatter: Formatter,
    django_filter: &Bound<'py, PyAny>,
    context: &mut Context,
) -> PyResult<Content<'t, 'py>> {
    let py = django_filter.py();
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
        let argument = resolve_django_argument(self.argument.as_ref(), py, template, context)?;
        call_django_filter(self.filter.bind(py), variable, argument, py, context)
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
        resolve_django_argument(self.argument.as_ref(), py, template, context)
    }

    fn call<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        argument: ResolvedArgument<'t, 'py>,
        py: Python<'py>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        call_django_filter(self.filter.bind(py), variable, argument, py, context)
    }
}

fn resolve_django_argument<'t, 'py>(
    argument: Option<&crate::types::Argument>,
    py: Python<'py>,
    template: TemplateString<'t>,
    context: &mut Context,
) -> Result<ResolvedArgument<'t, 'py>, PyRenderError> {
    Ok(match argument {
        None => ResolvedArgument::Absent,
        Some(argument) => ResolvedArgument::Present(argument.resolve(
            py,
            template,
            context,
            ResolveFailures::Raise,
        )?),
    })
}

/// A filter function's flag, as Django's `getattr(func, flag, False)`.
/// Django reads the flags on every call, so changes after compiling apply.
fn filter_flag(filter: &Bound<'_, PyAny>, name: &Bound<'_, PyString>) -> bool {
    filter
        .getattr_opt(name)
        .ok()
        .flatten()
        .is_some_and(|flag| flag.is_truthy().unwrap_or(false))
}

/// Calls a Django filter function as Django's `FilterExpression` does,
/// honouring its `expects_localtime`, `is_safe` and `needs_autoescape` flags.
fn call_django_filter<'t, 'py>(
    filter: &Bound<'py, PyAny>,
    variable: Option<Content<'t, 'py>>,
    argument: ResolvedArgument<'t, 'py>,
    py: Python<'py>,
    context: &mut Context,
) -> ResolveResult<'t, 'py> {
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
    if value.is_instance_of::<PyDateTime>() && filter_flag(filter, intern!(py, "expects_localtime"))
    {
        value = template_localtime(value, context.use_tz, &mut context.date_cache)?;
    }
    let safe_type = SAFEDATA.import(py, "django.utils.safestring", "SafeData")?;
    let preserve_safe =
        filter_flag(filter, intern!(py, "is_safe")) && value.is_instance(safe_type)?;
    let mut args = vec![value];
    match argument {
        ResolvedArgument::Absent => {}
        ResolvedArgument::Present(value) => {
            args.push(value.map_or_else(|| py.None().into_bound(py), |value| value.to_py(py)));
        }
    }
    let kwargs = if filter_flag(filter, intern!(py, "needs_autoescape")) {
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

/// The number formatting `django.utils.formats.number_format()` reads for
/// the active language.
#[derive(Clone, Debug)]
pub struct FloatNumberFormat {
    decimal_separator: String,
    thousand_separator: String,
    grouping: Grouping,
}

impl FloatNumberFormat {
    /// Returns `None` for settings Django would reject or treat specially,
    /// leaving those to Django.
    fn new(
        py: Python<'_>,
        language: &Bound<'_, PyString>,
        options: NumberOptions,
    ) -> PyResult<Option<Self>> {
        let get_format = GET_FORMAT.import(py, "django.utils.formats", "get_format")?;
        // Unlocalized formats come from settings, as `get_format(name, None,
        // use_l10n=False)` returns them.
        let format = |name| {
            if options.localize {
                get_format.call1((name, language, true))
            } else {
                get_format.call1((name, py.None(), false))
            }
        };
        let (Ok(decimal_separator), Ok(thousand_separator)) = (
            format("DECIMAL_SEPARATOR")?.extract(),
            format("THOUSAND_SEPARATOR")?.extract(),
        ) else {
            return Ok(None);
        };
        let grouping = format("NUMBER_GROUPING")?;
        // `numberformat.format()`: localized output follows
        // `USE_THOUSAND_SEPARATOR`, and `force_grouping` always groups.
        let use_grouping = (options.force_grouping
            || (options.localize
                && SETTINGS
                    .import(py, "django.conf", "settings")?
                    .getattr("USE_THOUSAND_SEPARATOR")?
                    .is_truthy()?))
            && grouping.ne(0)?;
        let grouping = if !use_grouping {
            Grouping::Off
        } else if grouping.is_exact_instance_of::<PyInt>() {
            match grouping.extract() {
                Ok(size) => Grouping::Sizes(vec![size]),
                Err(_) => return Ok(None),
            }
        } else if grouping.is_instance_of::<PyList>() || grouping.is_instance_of::<PyTuple>() {
            // An empty sequence makes Django raise IndexError.
            match grouping.extract::<Vec<i64>>() {
                Ok(sizes) if !sizes.is_empty() => Grouping::Sizes(sizes),
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
        self.grouping
            .push_digits(integer, &self.thousand_separator, &mut output);
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

/// `number_format()`'s `use_l10n` and `force_grouping`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NumberOptions {
    localize: bool,
    force_grouping: bool,
}

impl Default for NumberOptions {
    fn default() -> Self {
        Self {
            localize: true,
            force_grouping: false,
        }
    }
}

/// The number formats of one language, built for each [`NumberOptions`] on
/// first use. `None` marks settings left to Django.
#[derive(Clone, Debug)]
pub struct FloatNumberFormats {
    language: String,
    formats: HashMap<NumberOptions, Option<FloatNumberFormat>>,
}

impl FloatNumberFormats {
    fn new(language: &Bound<'_, PyString>) -> PyResult<Self> {
        Ok(Self {
            language: language.to_str()?.to_string(),
            formats: HashMap::new(),
        })
    }

    fn get(
        &mut self,
        py: Python<'_>,
        options: NumberOptions,
    ) -> PyResult<Option<&FloatNumberFormat>> {
        let format = match self.formats.entry(options) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(FloatNumberFormat::new(
                py,
                &PyString::new(py, &self.language),
                options,
            )?),
        };
        Ok(format.as_ref())
    }
}

/// The number formatting for the active language and `options`.
fn number_format<'c>(
    py: Python<'_>,
    context: &'c mut Context,
    options: NumberOptions,
) -> PyResult<Option<&'c FloatNumberFormat>> {
    match context
        .float_number_format
        .get_or_try_init(py, FloatNumberFormats::new)?
    {
        Some(formats) => formats.get(py, options),
        None => Ok(None),
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
/// `text` without a leading `+` when it is a plain decimal `Decimal()`
/// reads the same way as `DecimalNumber::parse`: an optional sign, ASCII
/// digits with an optional point, and an optional exponent.
fn decimal_text(text: &str) -> Option<&str> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if integer.len() + fraction.len() == 0 || !digits(integer) || !digits(fraction) {
        return None;
    }
    if let Some(exponent) = exponent {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if exponent.is_empty() || !digits(exponent) {
            return None;
        }
    }
    Some(text.strip_prefix('+').unwrap_or(text))
}

/// `floatformat`'s argument: the precision and the `g`/`u` suffix options.
/// `None` leaves the argument to Django.
fn floatformat_argument(argument: &ResolvedArgument<'_, '_>) -> Option<(i64, NumberOptions)> {
    let value = match argument {
        ResolvedArgument::Absent => return Some((-1, NumberOptions::default())),
        ResolvedArgument::Present(value) => value.as_ref()?,
    };
    let text = match value {
        Content::String(text) => text.as_raw().as_ref(),
        Content::Py(object)
            if object.is_exact_instance_of::<PyString>()
                || is_exact_safe_string(object).unwrap_or(false) =>
        {
            object.cast::<PyString>().ok()?.to_str().ok()?
        }
        other => return Some((integer_argument(other)?, NumberOptions::default())),
    };
    let mut options = NumberOptions::default();
    let precision = if let Some(rest) = text.strip_suffix("gu").or_else(|| text.strip_suffix("ug"))
    {
        options.force_grouping = true;
        options.localize = false;
        rest
    } else if let Some(rest) = text.strip_suffix('g') {
        options.force_grouping = true;
        rest
    } else if let Some(rest) = text.strip_suffix('u') {
        options.localize = false;
        rest
    } else if text.is_empty() {
        // Django's `arg[-1]` raises `IndexError`.
        return None;
    } else {
        return Some((python_int(text)?, options));
    };
    // `arg[:-1] or -1`.
    let precision = if precision.is_empty() {
        -1
    } else {
        python_int(precision)?
    };
    Some((precision, options))
}

fn floatformat(
    value: &Content<'_, '_>,
    precision: i64,
    options: NumberOptions,
    py: Python<'_>,
    context: &mut Context,
) -> PyResult<Option<String>> {
    let text = match value {
        // `Decimal(str(value))` for plain decimal strings; others, such as
        // `" 1"`, `"1_0"` and `"NaN"`, are left to Django.
        Content::String(text) => match decimal_text(text.as_raw()) {
            Some(text) => text.to_string(),
            None => return Ok(None),
        },
        Content::Py(object)
            if object.is_exact_instance_of::<PyString>()
                || is_exact_safe_string(object).unwrap_or(false) =>
        {
            match decimal_text(object.cast::<PyString>()?.to_str()?) {
                Some(text) => text.to_string(),
                None => return Ok(None),
            }
        }
        // `Decimal("True")` and `float("True")` fail, so Django returns "".
        Content::Bool(_) => return Ok(Some(String::new())),
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
        Content::Py(_) => return Ok(None),
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
    let Some(format) = number_format(py, context, options)? else {
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
        if let Some((precision, options)) = floatformat_argument(&argument)
            && let Some(value) = &variable
            && let Some(formatted) = floatformat(value, precision, options, py, context)?
        {
            return Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
                formatted,
            )))));
        }
        self.django.call(variable, argument, py, context)
    }
}

/// What Django's `join` returns for a list or tuple.
enum Joined {
    /// `mark_safe` of the joined text.
    Text(String),
    /// The value itself: joining raised `TypeError`, which Django's filter
    /// swallows.
    Unchanged,
}

/// Whether `object` is exactly a `SafeString`, so `conditional_escape`
/// returns it as it is.
fn is_exact_safe_string(object: &Bound<'_, PyAny>) -> PyResult<bool> {
    let py = object.py();
    Ok(object
        .get_type()
        .is(SAFESTRING.import(py, "django.utils.safestring", "SafeString")?))
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

impl ResolveFilter for TimesinceFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some(text) =
            time_since_text(variable.as_ref(), argument.value(), self.until, context)?
        {
            return Ok(Some(Content::String(match context.autoescape {
                false => ContentString::String(Cow::Owned(text)),
                true => ContentString::HtmlUnsafe(Cow::Owned(text)),
            })));
        }
        self.django.call(variable, argument, py, context)
    }
}

/// What Django's `timesince_filter`, or `timeuntil_filter` when `until` is
/// set, returns for `value` and its comparison time `now`. `None` when
/// Django's filter must handle them.
fn time_since_text(
    value: Option<&Content<'_, '_>>,
    now: Option<&Content<'_, '_>>,
    until: bool,
    context: &mut Context,
) -> PyResult<Option<String>> {
    // Both filters return "" for a falsy value and treat a falsy comparison
    // time as the current time.
    let value = match value {
        None => return Ok(Some(String::new())),
        Some(Content::Py(value)) if value.is_none() => return Ok(Some(String::new())),
        Some(Content::Py(value)) => value,
        Some(Content::String(
            ContentString::String(text)
            | ContentString::HtmlSafe(text)
            | ContentString::HtmlUnsafe(text),
        )) if text.is_empty() => return Ok(Some(String::new())),
        Some(_) => return Ok(None),
    };
    let now = match now {
        None => None,
        Some(Content::Py(now)) if now.is_none() => None,
        Some(Content::Py(now)) => Some(now),
        Some(Content::String(
            ContentString::String(text)
            | ContentString::HtmlSafe(text)
            | ContentString::HtmlUnsafe(text),
        )) if text.is_empty() => None,
        Some(_) => return Ok(None),
    };
    // Django's filter reproduces any error, or catches it and returns "".
    let Ok(Some(since)) = timesince(value, now, until) else {
        return Ok(None);
    };
    let partials = match since {
        TimeSince::Partials(partials) => partials,
        TimeSince::Mismatched => return Ok(Some(String::new())),
    };
    let py = value.py();
    let text = match context
        .time_strings
        .get_or_try_init(py, |_| Ok(TimeStrings::default()))?
    {
        Some(strings) => strings.format(py, partials)?,
        None => TimeStrings::default().format(py, partials)?,
    };
    Ok(Some(text))
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
fn is_python_whitespace(character: char) -> bool {
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

impl ResolveFilter for GetDigitFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        // Django converts both with `int()`. A negative value can index its
        // minus sign, which raises, so those stay with Django.
        if let Some(position) = argument.value().and_then(integer_argument)
            && let Some(value) = variable.as_ref().and_then(integer_argument)
            && value >= 0
        {
            let digits = value.to_string();
            let digit = match usize::try_from(position) {
                // Django returns the converted value for a position below 1.
                Err(_) | Ok(0) => value,
                Ok(position) => digits
                    .len()
                    .checked_sub(position)
                    .map_or(0, |index| i64::from(digits.as_bytes()[index] - b'0')),
            };
            return Ok(Some(Content::Int(digit.into())));
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
static PFORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

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

impl ResolveFilter for PprintFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Django marks the result safe when the value was, so safe strings
        // go through Django's filter.
        let value = match &variable {
            Some(Content::String(ContentString::HtmlSafe(_))) => None,
            Some(Content::Py(value))
                if value.is_instance(SAFEDATA.import(
                    py,
                    "django.utils.safestring",
                    "SafeData",
                )?)? =>
            {
                None
            }
            Some(value) => Some(value.to_py(py)),
            // Django's filter gets "".
            None => Some(PyString::new(py, "").into_any()),
        };
        let Some(value) = value else {
            return self.django.resolve(variable, py, template, context);
        };
        let formatted = match PFORMAT.import(py, "pprint", "pformat")?.call1((value,)) {
            Ok(formatted) => formatted.str()?.to_str()?.to_string(),
            Err(error) if error.is_instance_of::<pyo3::exceptions::PyException>(py) => format!(
                "Error in formatting: {}: {}",
                error.get_type(py).name()?,
                error.value(py).str()?
            ),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(Content::String(ContentString::HtmlUnsafe(
            Cow::Owned(formatted),
        ))))
    }
}

static NGETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static GETTEXT_FUNCTION: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// The `filesizeformat` units above bytes, as Django's translatable strings.
const FILE_SIZE_UNITS: [&str; 5] = ["%s KB", "%s MB", "%s GB", "%s TB", "%s PB"];

/// A translated `"%s KB"`-style template.
#[derive(Clone, Debug)]
enum UnitTemplate {
    /// Split around its only `%s`.
    Split(String, String),
    /// Any other template, %-formatted by Python.
    Python(String),
}

/// `filesizeformat`'s translated strings for one language.
#[derive(Clone, Debug, Default)]
pub struct FileSizeStrings {
    /// `ngettext("%(size)d byte", "%(size)d bytes", n) % {"size": n}`.
    bytes: HashMap<u64, String>,
    units: [Option<UnitTemplate>; 5],
}

impl FileSizeStrings {
    fn bytes(&mut self, py: Python<'_>, count: u64) -> PyResult<&str> {
        Ok(match self.bytes.entry(count) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let size = PyDict::new(py);
                size.set_item("size", count)?;
                let text = NGETTEXT
                    .import(py, "django.utils.translation", "ngettext")?
                    .call1(("%(size)d byte", "%(size)d bytes", count))?
                    .rem(size)?;
                entry.insert(text.extract()?)
            }
        })
    }

    fn unit(&mut self, py: Python<'_>, index: usize, number: &str) -> PyResult<String> {
        let template = match &mut self.units[index] {
            Some(template) => template,
            slot @ None => {
                let text: String = GETTEXT_FUNCTION
                    .import(py, "django.utils.translation", "gettext")?
                    .call1((FILE_SIZE_UNITS[index],))?
                    .extract()?;
                slot.insert(match text.split_once("%s") {
                    Some((before, after)) if text.matches('%').count() == 1 => {
                        UnitTemplate::Split(before.to_string(), after.to_string())
                    }
                    _ => UnitTemplate::Python(text),
                })
            }
        };
        Ok(match template {
            UnitTemplate::Split(before, after) => format!("{before}{number}{after}"),
            UnitTemplate::Python(text) => PyString::new(py, text).rem(number)?.extract()?,
        })
    }
}

/// Python's `round(n / divisor, 1)` for `n < 2⁵³`, where the division is
/// exact: the nearest tenth, ties to even. Returns the integer part and the
/// tenths digit.
fn round_tenths(n: u64, divisor: u64) -> (u64, u64) {
    let scaled = n * 10;
    let mut tenths = scaled / divisor;
    let remainder = scaled % divisor;
    if remainder * 2 > divisor || (remainder * 2 == divisor && tenths % 2 == 1) {
        tenths += 1;
    }
    (tenths / 10, tenths % 10)
}

impl ResolveFilter for FilesizeformatFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Django converts with `int()`; failures give "0 bytes", left to it.
        // Above 2⁵³, `bytes / unit` isn't exact.
        let Some(bytes) = variable.as_ref().and_then(integer_argument) else {
            return self.django.resolve(variable, py, template, context);
        };
        let size = bytes.unsigned_abs();
        if size >= 1 << 53 {
            return self.django.resolve(variable, py, template, context);
        }
        let safe = match &variable {
            Some(Content::String(ContentString::HtmlSafe(_))) => true,
            Some(Content::Py(value)) => {
                value.is_instance(SAFEDATA.import(py, "django.utils.safestring", "SafeData")?)?
            }
            _ => false,
        };
        let text = if size < 1 << 10 {
            let Some(strings) = context
                .file_size_strings
                .get_or_try_init(py, |_| Ok(FileSizeStrings::default()))?
            else {
                return self.django.resolve(variable, py, template, context);
            };
            strings.bytes(py, size)?.to_string()
        } else {
            let index = (63 - size.leading_zeros() as usize) / 10 - 1;
            let index = index.min(FILE_SIZE_UNITS.len() - 1);
            let (integer, tenth) = round_tenths(size, 1 << (10 * (index + 1)));
            // `formats.number_format(value, 1)`, localized like `floatformat`.
            let Some(format) = number_format(py, context, NumberOptions::default())? else {
                return self.django.resolve(variable, py, template, context);
            };
            let number = format.format(&Rounded {
                sign: Sign::Plus,
                integer: integer.to_string(),
                fraction: tenth.to_string(),
            });
            let Some(strings) = context
                .file_size_strings
                .get_or_try_init(py, |_| Ok(FileSizeStrings::default()))?
            else {
                return self.django.resolve(variable, py, template, context);
            };
            strings.unit(py, index, &number)?
        };
        // `"-%s" % value` for negative sizes, then `avoid_wrapping()`.
        let sign = if bytes < 0 { "-" } else { "" };
        let text = format!("{sign}{}", text.replace(' ', "\u{a0}"));
        Ok(Some(Content::String(if safe {
            ContentString::HtmlSafe(Cow::Owned(text))
        } else {
            ContentString::HtmlUnsafe(Cow::Owned(text))
        })))
    }
}

/// Nesting beyond this goes to Django, which raises `RecursionError` for
/// lists that contain themselves.
const MAX_LIST_DEPTH: usize = 100;

/// The items of an exact list or tuple. Subclasses and other iterables
/// return `None`, leaving them to Django.
fn exact_sequence<'py>(value: &Bound<'py, PyAny>) -> Option<Vec<Bound<'py, PyAny>>> {
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

/// Elements whose content Python's HTML parser reads as raw text.
const RAW_TEXT_ELEMENTS: [&str; 10] = [
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
fn format_spec_text<'a>(argument: &'a Content<'_, '_>) -> Option<&'a str> {
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

/// JSON nesting beyond this goes to Django, which detects circular
/// references.
const MAX_JSON_DEPTH: usize = 100;

/// Python's `repr()` of a finite float, as `json.dumps` writes it: the
/// shortest round-trip digits, in exponent form below 1e-4 and from 1e16.
fn python_float_repr(value: f64) -> String {
    // Rust's shortest digits break exact ties upward; Python rounds them to
    // even, as Rust does when formatting to that many digits.
    let shortest = format!("{value:e}");
    let precision = shortest
        .split_once('e')
        .map_or(0, |(mantissa, _)| {
            mantissa.bytes().filter(u8::is_ascii_digit).count()
        })
        .saturating_sub(1);
    let formatted = format!("{value:.precision$e}");
    let (mantissa, exponent) = formatted.split_once('e').unwrap_or((&formatted, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let (sign, mantissa) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |mantissa| ("-", mantissa));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let mut output = String::from(sign);
    if (-4..16).contains(&exponent) {
        if let Ok(point) = usize::try_from(exponent + 1)
            && point > 0
        {
            if digits.len() > point {
                output.push_str(&digits[..point]);
                output.push('.');
                output.push_str(&digits[point..]);
            } else {
                output.push_str(&digits);
                output.extend(std::iter::repeat_n('0', point - digits.len()));
                output.push_str(".0");
            }
        } else {
            output.push_str("0.");
            output.extend(std::iter::repeat_n(
                '0',
                exponent.unsigned_abs() as usize - 1,
            ));
            output.push_str(&digits);
        }
    } else {
        output.push_str(&digits[..1]);
        if digits.len() > 1 {
            output.push('.');
            output.push_str(&digits[1..]);
        }
        let sign = if exponent < 0 { '-' } else { '+' };
        output.push_str(&format!("e{sign}{:02}", exponent.unsigned_abs()));
    }
    output
}

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

static HTML_MODULE: PyOnceLock<Py<PyModule>> = PyOnceLock::new();

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

/// Django's `urlize(text, trim_url_limit, nofollow=True, autoescape)`.
/// Words that can't become links are escaped as `handle_word` does; the
/// rest go through Django's `urlizer`, once per distinct word.
fn urlize_text(
    text: &str,
    safe: bool,
    limit: Option<i64>,
    autoescape: bool,
    py: Python<'_>,
) -> PyResult<String> {
    // `word_split_re`: runs of whitespace, `<`, `>`, `"` and `'` separate words.
    let is_separator =
        |character: char| is_python_whitespace(character) || "<>\"'".contains(character);
    let mut output = String::with_capacity(text.len());
    let mut links: HashMap<&str, String> = HashMap::new();
    let mut urlizer = None;
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
            let output = urlize_text(text.as_str(), safe, None, context.autoescape, py)?;
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
            let output = urlize_text(text.as_str(), safe, Some(limit), context.autoescape, py)?;
            return Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
                output,
            )))));
        }
        self.django.call(variable, argument, py, context)
    }
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
