//! Date, time, localization and time zone filters.

use std::borrow::Cow;
use std::collections::HashMap;

use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyString;

use super::ResolveFilter;
use crate::error::AnnotatePyErr;
use crate::filters::{DateFilter, L10nFilter, TimesinceFilter, TzFilter};
use crate::filters::{LanguageField, LanguageInfoFilter};
use crate::render::dateformat::{
    Formatted, Formatter, convert_timezone, localize_date_value, template_localtime,
};
use crate::render::l10n::localize_number;
use crate::render::language_cache::LanguageInfo;
use crate::render::timesince::{TimeSince, TimeStrings, timesince};
use crate::render::types::{AsBorrowedContent, Content, ContentString, Context};
use crate::render::{Resolve, ResolveFailures, ResolveResult};
use dtl_lexer::types::TemplateString;

pub static DATE_FILTER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static GETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

impl ResolveFilter for DateFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Resolved first: a missing argument raises even for a missing value.
        let format = match &self.argument {
            Some(arg) => Some(
                arg.resolve(py, template, context, ResolveFailures::Raise)?
                    .expect("missing argument in context should already have raised"),
            ),
            None => None,
        };
        let Some(value) = variable else {
            return Ok(Some("".as_content()));
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
pub(super) fn format_date<'t, 'py>(
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
    let value = template_localtime(value, context.use_tz, &mut context.date_cache.borrow_mut())?;
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
        && let Some(formatted) =
            context
                .date_cache
                .borrow_mut()
                .format(&value, &format_name, formatter)?
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

impl LanguageInfoFilter {
    /// The filter's result for `code`, or `None` to call Django's filter.
    fn native<'t, 'py>(
        &self,
        code: &str,
        py: Python<'py>,
        context: &mut Context,
    ) -> PyResult<Option<Content<'t, 'py>>> {
        let info = match context.caches().language_info.get(code) {
            Some(info) => info,
            None => match LanguageInfo::load(py, code)? {
                Some(info) => context
                    .caches()
                    .language_info
                    .entry(code.to_owned())
                    .or_insert(info),
                None => return Ok(None),
            },
        };
        let text = match self.field {
            LanguageField::Name => info.name.clone(),
            LanguageField::NameLocal => info.name_local.clone(),
            LanguageField::Bidi => return Ok(Some(Content::Bool(info.bidi))),
            LanguageField::NameTranslated => {
                let name = info.name.clone();
                let Some(names) = context
                    .caches()
                    .translated_language_names
                    .get_or_try_init(py, |_| Ok(HashMap::new()))?
                else {
                    return Ok(None);
                };
                match names.get(code) {
                    Some(translated) => translated.clone(),
                    None => {
                        let translated: String = GETTEXT
                            .import(py, "django.utils.translation", "gettext")?
                            .call1((&name,))?
                            .str()?
                            .extract()?;
                        names.insert(code.to_owned(), translated.clone());
                        translated
                    }
                }
            }
        };
        Ok(Some(Content::String(ContentString::HtmlUnsafe(
            Cow::Owned(text),
        ))))
    }
}

impl ResolveFilter for LanguageInfoFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let code = match &variable {
            Some(Content::String(string)) => Some(string.as_raw().to_string()),
            Some(Content::Py(object)) if object.is_exact_instance_of::<PyString>() => {
                Some(object.extract::<String>()?)
            }
            _ => None,
        };
        if let Some(code) = code
            && let Some(content) = self.native(&code, py, context)?
        {
            return Ok(Some(content));
        }
        let argument = self.django.resolve_argument(py, template, context)?;
        self.django.call(variable, argument, py, context)
    }
}

impl ResolveFilter for L10nFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let use_l10n = if self.localize { None } else { Some(false) };
        let value = match variable {
            // Missing variables reach Django's filter as "".
            None => return Ok(Some("".as_content())),
            // `localize` returns strings unchanged, and `str()` keeps them.
            Some(Content::String(string)) => return Ok(Some(Content::String(string))),
            Some(Content::Bool(value)) => {
                let text = if value { "True" } else { "False" };
                return Ok(Some(text.as_content()));
            }
            Some(content) => content,
        };
        let object = value.to_py(py);
        if object.is_exact_instance_of::<PyString>() {
            return Ok(Some(Content::Py(object)));
        }
        if let Some(number) = localize_number(&object, use_l10n, context)? {
            return Ok(Some(Content::String(ContentString::HtmlUnsafe(
                Cow::Owned(number),
            ))));
        }
        // `unlocalize` reads the format settings, not the locale's, for dates.
        if self.localize
            && let Some(formatted) =
                localize_date_value(&object, &mut context.date_cache.borrow_mut())?
        {
            return Ok(Some(Content::String(ContentString::HtmlUnsafe(
                Cow::Owned(formatted),
            ))));
        }
        let argument = self.django.resolve_argument(py, template, context)?;
        self.django.call(Some(value), argument, py, context)
    }
}

impl ResolveFilter for TzFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        let converted = convert_timezone(
            self.conversion,
            variable.as_ref(),
            argument.value(),
            py,
            &mut context.date_cache.borrow_mut(),
        )?;
        match converted {
            Some(Some(value)) => Ok(Some(Content::Py(value))),
            Some(None) => Ok(Some("".as_content())),
            None => self.django.call(variable, argument, py, context),
        }
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
        .caches()
        .time_strings
        .get_or_try_init(py, |_| Ok(TimeStrings::default()))?
    {
        Some(strings) => strings.format(py, partials)?,
        None => TimeStrings::default().format(py, partials)?,
    };
    Ok(Some(text))
}
