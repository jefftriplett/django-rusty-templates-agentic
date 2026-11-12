//! Filters implemented by calling Django, and the argument handling they share.

use std::borrow::Cow;

use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyDateTime, PyString};

use super::date::format_date;
use super::{MARK_FILTER_SAFE, ResolveFilter, SAFEDATA};
use crate::error::PyRenderError;
use crate::filters::{DjangoFilter, ExternalFilter, PprintFilter};
use crate::render::dateformat::{Formatter, template_localtime};
use crate::render::types::{AsBorrowedContent, Content, ContentString, Context};
use crate::render::{Resolve, ResolveFailures, ResolveResult};
use dtl_lexer::types::TemplateString;

static TIME_FILTER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

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
pub enum ResolvedArgument<'t, 'py> {
    /// The filter was used without an argument.
    Absent,
    /// The argument's value, `None` when it resolved to nothing. Django's
    /// filter then receives `None`.
    Present(Option<Content<'t, 'py>>),
}

impl<'t, 'py> ResolvedArgument<'t, 'py> {
    pub(crate) fn value(&self) -> Option<&Content<'t, 'py>> {
        match self {
            Self::Absent => None,
            Self::Present(value) => value.as_ref(),
        }
    }

    pub(crate) fn into_value(self) -> Option<Content<'t, 'py>> {
        match self {
            Self::Absent => None,
            Self::Present(value) => value,
        }
    }
}

impl DjangoFilter {
    pub(crate) fn resolve_argument<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<ResolvedArgument<'t, 'py>, PyRenderError> {
        resolve_django_argument(self.argument.as_ref(), py, template, context)
    }

    pub(crate) fn call<'t, 'py>(
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
        value = template_localtime(value, context.use_tz, &mut context.date_cache.borrow_mut())?;
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

static PFORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

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
