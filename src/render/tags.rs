use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use num_bigint::{BigInt, Sign};
use num_traits::cast::ToPrimitive;
use pyo3::exceptions::{
    PyAttributeError, PyOverflowError, PyTypeError, PyValueError, PyZeroDivisionError,
};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::{MutexExt, PyOnceLock};
use pyo3::types::{PyBool, PyDict, PyList, PyNone, PyString, PyTuple};

use crate::parse::Now;
use crate::render::filters::ResolveFilter;
use crate::render::lorem::{COMMON_WORDS, paragraphs, words};
use dtl_lexer::tag::lorem::LoremMethod;
use dtl_lexer::types::{At, TemplateString};

use super::types::{
    AsBorrowedContent, Content, ContentString, Context, IncludeTemplateKey, PyContext,
};
use super::{Evaluate, Render, RenderResult, Resolve, ResolveFailures, ResolveResult};
use crate::error::{AnnotatePyErr, PyRenderError, RenderError};
use crate::parse::{
    CsrfToken, Cycle, CycleValue, FirstOf, For, IfCondition, Include, IncludeTemplateName,
    InclusionTag, Lorem, NamedCycle, SilentNamedCycle, SimpleBlockTag, SimpleCycle, SimpleTag, Tag,
    TagElement, Url,
};
use crate::path::construct_relative_path;
use crate::template::django_rusty_templates::{
    NoReverseMatch, Template, TemplateDoesNotExist, TemplateSyntaxError,
};
use crate::utils::PyResultMethods;

static RENDER_FIRSTOF_VALUE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static STRIP_SPACES: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DEBUG_NODE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static MARK_BLOCK_SAFE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

static BUILTINS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

static PROMISE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static REVERSE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static WARNINGS_WARN: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DJANGO_DATEFORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DJANGO_FORMATS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DJANGO_SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DJANGO_TIMEZONE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

fn current_app(py: Python, request: Option<&Py<PyAny>>) -> PyResult<Py<PyAny>> {
    let Some(request) = request else {
        return Ok(py.None());
    };
    if let Ok(current_app) = request
        .getattr(py, "current_app")
        .ok_or_isinstance_of::<PyAttributeError>(py)?
    {
        return Ok(current_app);
    }
    match request
        .getattr(py, "resolver_match")
        .ok_or_isinstance_of::<PyAttributeError>(py)?
    {
        Ok(resolver_match) if !resolver_match.is_none(py) => {
            resolver_match.getattr(py, "namespace")
        }
        _ => Ok(py.None()),
    }
}

impl Resolve for Url {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
        failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        let view_name = match self.view_name.resolve(py, template, context, failures)? {
            Some(view_name) => view_name,
            None => "".as_content(),
        };
        let reverse = REVERSE.import(py, "django_rusty_templates.diagnostics", "render_url")?;

        let current_app = current_app(py, context.request.as_ref())?;
        let url = if self.kwargs.is_empty() {
            let py_args = PyList::empty(py);
            for arg in &self.args {
                py_args.append(
                    arg.resolve(py, template, context, failures)?
                        .unwrap_or("".as_content()),
                )?;
            }
            reverse.call1((
                view_name,
                py.None(),
                py_args.to_tuple(),
                py.None(),
                current_app,
            ))
        } else {
            let kwargs = PyDict::new(py);
            for (key, value) in &self.kwargs {
                kwargs.set_item(key, value.resolve(py, template, context, failures)?)?;
            }
            reverse.call1((view_name, py.None(), py.None(), kwargs, current_app))
        };
        let url = match url {
            Ok(url) => Ok(url),
            Err(error) => Err(error.annotate(py, self.at, "here", template)),
        };
        match &self.asvar {
            None => Ok(Some(Content::Py(url?))),
            Some(variable) => {
                let value = match url.ok_or_isinstance_of::<NoReverseMatch>(py)? {
                    Ok(url) => url,
                    Err(_) => PyString::new(py, "").into_any(),
                };
                context.insert(variable.clone(), value)?;
                Ok(None)
            }
        }
    }
}

impl Evaluate for Content<'_, '_> {
    fn evaluate(
        &self,
        _py: Python<'_>,
        _template: TemplateString<'_>,
        _context: &mut Context,
    ) -> Option<bool> {
        Some(match self {
            Self::Py(obj) => obj.is_truthy().unwrap_or(false),
            Self::String(s) => !s.as_raw().is_empty(),
            Self::Float(f) => *f != 0.0,
            Self::Int(n) => *n != BigInt::ZERO,
            Self::Bool(b) => *b,
        })
    }
}

trait PyCmp<T> {
    fn eq(&self, other: &T) -> bool;

    fn ne(&self, other: &T) -> bool {
        !self.eq(other)
    }

    fn lt(&self, other: &T) -> bool;

    fn gt(&self, other: &T) -> bool;

    fn lte(&self, other: &T) -> bool;

    fn gte(&self, other: &T) -> bool;
}

impl PyCmp<Content<'_, '_>> for Content<'_, '_> {
    fn eq(&self, other: &Content<'_, '_>) -> bool {
        match (self, other) {
            (Self::Py(obj), Content::Py(other)) => obj.eq(other).unwrap_or(false),
            (Self::Py(obj), Content::Float(other)) => obj.eq(other).unwrap_or(false),
            (Self::Py(obj), Content::Int(other)) => obj.eq(other).unwrap_or(false),
            (Self::Py(obj), Content::Bool(other)) => obj.eq(other).unwrap_or(false),
            (Self::Py(obj), Content::String(other)) => obj.eq(other.as_raw()).unwrap_or(false),
            (Self::Float(obj), Content::Py(other)) => other.eq(obj).unwrap_or(false),
            (Self::Int(obj), Content::Py(other)) => other.eq(obj).unwrap_or(false),
            (Self::String(obj), Content::Py(other)) => other.eq(obj.as_raw()).unwrap_or(false),
            (Self::Bool(obj), Content::Py(other)) => other.eq(obj).unwrap_or(false),
            (Self::Float(obj), Content::Float(other)) => obj == other,
            (Self::Int(obj), Content::Int(other)) => obj == other,
            (Self::Int(obj), Content::Bool(other)) => u8::try_from(obj)
                .map(|o| o == u8::from(*other))
                .unwrap_or(false),
            (Self::Bool(obj), Content::Int(other)) => u8::try_from(other)
                .map(|o| o == u8::from(*obj))
                .unwrap_or(false),
            (Self::Float(obj), Content::Int(other)) => {
                match other.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY | f64::NEG_INFINITY => false,
                    other => *obj == other,
                }
            }
            (Self::Int(obj), Content::Float(other)) => {
                match obj.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY | f64::NEG_INFINITY => false,
                    obj => obj == *other,
                }
            }
            (Self::Float(obj), Content::Bool(other)) => match other {
                true => *obj == 1.0,
                false => *obj == 0.0,
            },
            (Self::Bool(obj), Content::Float(other)) => match obj {
                true => *other == 1.0,
                false => *other == 0.0,
            },
            (Self::String(obj), Content::String(other)) => obj.as_raw() == other.as_raw(),
            (Self::Bool(obj), Content::Bool(other)) => obj == other,
            _ => false,
        }
    }

    fn lt(&self, other: &Content<'_, '_>) -> bool {
        match (self, other) {
            (Self::Py(obj), Content::Py(other)) => obj.lt(other).unwrap_or(false),
            (Self::Py(obj), Content::Float(other)) => obj.lt(other).unwrap_or(false),
            (Self::Py(obj), Content::Int(other)) => obj.lt(other).unwrap_or(false),
            (Self::Py(obj), Content::Bool(other)) => obj.lt(other).unwrap_or(false),
            (Self::Py(obj), Content::String(other)) => obj.lt(other.as_raw()).unwrap_or(false),
            (Self::Float(obj), Content::Py(other)) => other.gt(obj).unwrap_or(false),
            (Self::Int(obj), Content::Py(other)) => other.gt(obj).unwrap_or(false),
            (Self::String(obj), Content::Py(other)) => other.gt(obj.as_raw()).unwrap_or(false),
            (Self::Bool(obj), Content::Py(other)) => other.gt(obj).unwrap_or(false),
            (Self::Float(obj), Content::Float(other)) => obj < other,
            (Self::Int(obj), Content::Int(other)) => obj < other,
            (Self::Int(obj), Content::Bool(other)) => match obj.sign() {
                Sign::Minus => true,
                _ => u8::try_from(obj)
                    .map(|o| o < u8::from(*other))
                    .unwrap_or(false),
            },
            (Self::Bool(obj), Content::Int(other)) => match other.sign() {
                Sign::Minus => false,
                _ => u8::try_from(other)
                    .map(|o| o > u8::from(*obj))
                    .unwrap_or(true),
            },
            (Self::Float(obj), Content::Int(other)) => {
                match other.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY => obj.is_finite() || *obj == f64::NEG_INFINITY,
                    f64::NEG_INFINITY => *obj == f64::NEG_INFINITY,
                    other => *obj < other,
                }
            }
            (Self::Int(obj), Content::Float(other)) => {
                match obj.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY => *other == f64::INFINITY,
                    f64::NEG_INFINITY => other.is_finite() || *other == f64::INFINITY,
                    obj => obj < *other,
                }
            }
            (Self::Float(obj), Content::Bool(other)) => match other {
                true => *obj < 1.0,
                false => *obj < 0.0,
            },
            (Self::Bool(obj), Content::Float(other)) => match obj {
                true => *other > 1.0,
                false => *other > 0.0,
            },
            (Self::String(obj), Content::String(other)) => obj.as_raw() < other.as_raw(),
            (Self::Bool(obj), Content::Bool(other)) => obj < other,
            _ => false,
        }
    }

    fn gt(&self, other: &Content<'_, '_>) -> bool {
        match (self, other) {
            (Self::Py(obj), Content::Py(other)) => obj.gt(other).unwrap_or(false),
            (Self::Py(obj), Content::Float(other)) => obj.gt(other).unwrap_or(false),
            (Self::Py(obj), Content::Int(other)) => obj.gt(other).unwrap_or(false),
            (Self::Py(obj), Content::Bool(other)) => obj.gt(other).unwrap_or(false),
            (Self::Py(obj), Content::String(other)) => obj.gt(other.as_raw()).unwrap_or(false),
            (Self::Float(obj), Content::Py(other)) => other.lt(obj).unwrap_or(false),
            (Self::Int(obj), Content::Py(other)) => other.lt(obj).unwrap_or(false),
            (Self::String(obj), Content::Py(other)) => other.lt(obj.as_raw()).unwrap_or(false),
            (Self::Bool(obj), Content::Py(other)) => other.lt(obj).unwrap_or(false),
            (Self::Float(obj), Content::Float(other)) => obj > other,
            (Self::Int(obj), Content::Int(other)) => obj > other,
            (Self::Int(obj), Content::Bool(other)) => match obj.sign() {
                Sign::Minus => false,
                _ => u8::try_from(obj)
                    .map(|o| o > u8::from(*other))
                    .unwrap_or(true),
            },
            (Self::Bool(obj), Content::Int(other)) => match other.sign() {
                Sign::Minus => true,
                _ => u8::try_from(other)
                    .map(|o| o < u8::from(*obj))
                    .unwrap_or(false),
            },
            (Self::Float(obj), Content::Int(other)) => {
                match other.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY => *obj == f64::INFINITY,
                    f64::NEG_INFINITY => obj.is_finite() || *obj == f64::INFINITY,
                    other => *obj > other,
                }
            }
            (Self::Int(obj), Content::Float(other)) => {
                match obj.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY => other.is_finite() || *other == f64::NEG_INFINITY,
                    f64::NEG_INFINITY => *other == f64::NEG_INFINITY,
                    obj => obj > *other,
                }
            }
            (Self::Float(obj), Content::Bool(other)) => match other {
                true => *obj > 1.0,
                false => *obj > 0.0,
            },
            (Self::Bool(obj), Content::Float(other)) => match obj {
                true => *other < 1.0,
                false => *other < 0.0,
            },
            (Self::String(obj), Content::String(other)) => obj.as_raw() > other.as_raw(),
            (Self::Bool(obj), Content::Bool(other)) => obj > other,
            _ => false,
        }
    }

    fn lte(&self, other: &Content<'_, '_>) -> bool {
        match (self, other) {
            (Self::Py(obj), Content::Py(other)) => obj.le(other).unwrap_or(false),
            (Self::Py(obj), Content::Float(other)) => obj.le(other).unwrap_or(false),
            (Self::Py(obj), Content::Int(other)) => obj.le(other).unwrap_or(false),
            (Self::Py(obj), Content::Bool(other)) => obj.le(other).unwrap_or(false),
            (Self::Py(obj), Content::String(other)) => obj.le(other.as_raw()).unwrap_or(false),
            (Self::Float(obj), Content::Py(other)) => other.ge(obj).unwrap_or(false),
            (Self::Int(obj), Content::Py(other)) => other.ge(obj).unwrap_or(false),
            (Self::Bool(obj), Content::Py(other)) => other.ge(obj).unwrap_or(false),
            (Self::String(obj), Content::Py(other)) => other.ge(obj.as_raw()).unwrap_or(false),
            (Self::Float(obj), Content::Float(other)) => obj <= other,
            (Self::Int(obj), Content::Int(other)) => obj <= other,
            (Self::Int(obj), Content::Bool(other)) => match obj.sign() {
                Sign::Minus => true,
                _ => u8::try_from(obj)
                    .map(|o| o <= u8::from(*other))
                    .unwrap_or(false),
            },
            (Self::Bool(obj), Content::Int(other)) => match other.sign() {
                Sign::Minus => false,
                _ => u8::try_from(other)
                    .map(|o| o >= u8::from(*obj))
                    .unwrap_or(true),
            },
            (Self::Float(obj), Content::Int(other)) => {
                match other.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY => obj.is_finite() || *obj == f64::NEG_INFINITY,
                    f64::NEG_INFINITY => *obj == f64::NEG_INFINITY,
                    other => *obj <= other,
                }
            }
            (Self::Int(obj), Content::Float(other)) => {
                match obj.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY => *other == f64::INFINITY,
                    f64::NEG_INFINITY => other.is_finite() || *other == f64::INFINITY,
                    obj => obj <= *other,
                }
            }
            (Self::Float(obj), Content::Bool(other)) => match other {
                true => *obj <= 1.0,
                false => *obj <= 0.0,
            },
            (Self::Bool(obj), Content::Float(other)) => match obj {
                true => *other >= 1.0,
                false => *other >= 0.0,
            },
            (Self::String(obj), Content::String(other)) => obj.as_raw() <= other.as_raw(),
            (Self::Bool(obj), Content::Bool(other)) => obj <= other,
            _ => false,
        }
    }

    fn gte(&self, other: &Content<'_, '_>) -> bool {
        match (self, other) {
            (Self::Py(obj), Content::Py(other)) => obj.ge(other).unwrap_or(false),
            (Self::Py(obj), Content::Float(other)) => obj.ge(other).unwrap_or(false),
            (Self::Py(obj), Content::Int(other)) => obj.ge(other).unwrap_or(false),
            (Self::Py(obj), Content::Bool(other)) => obj.ge(other).unwrap_or(false),
            (Self::Py(obj), Content::String(other)) => obj.ge(other.as_raw()).unwrap_or(false),
            (Self::Float(obj), Content::Py(other)) => other.le(obj).unwrap_or(false),
            (Self::Int(obj), Content::Py(other)) => other.le(obj).unwrap_or(false),
            (Self::Bool(obj), Content::Py(other)) => other.le(obj).unwrap_or(false),
            (Self::String(obj), Content::Py(other)) => other.le(obj.as_raw()).unwrap_or(false),
            (Self::Float(obj), Content::Float(other)) => obj >= other,
            (Self::Int(obj), Content::Int(other)) => obj >= other,
            (Self::Int(obj), Content::Bool(other)) => match obj.sign() {
                Sign::Minus => false,
                _ => u8::try_from(obj)
                    .map(|o| o >= u8::from(*other))
                    .unwrap_or(true),
            },
            (Self::Bool(obj), Content::Int(other)) => match other.sign() {
                Sign::Minus => true,
                _ => u8::try_from(other)
                    .map(|o| o <= u8::from(*obj))
                    .unwrap_or(false),
            },
            (Self::Float(obj), Content::Int(other)) => {
                match other.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY => *obj == f64::INFINITY,
                    f64::NEG_INFINITY => obj.is_finite() || *obj == f64::INFINITY,
                    other => *obj >= other,
                }
            }
            (Self::Int(obj), Content::Float(other)) => {
                match obj.to_f64().expect("BigInt to f64 is always possible") {
                    f64::INFINITY => other.is_finite() || *other == f64::NEG_INFINITY,
                    f64::NEG_INFINITY => *other == f64::NEG_INFINITY,
                    obj => obj >= *other,
                }
            }
            (Self::Float(obj), Content::Bool(other)) => match other {
                true => *obj >= 1.0,
                false => *obj >= 0.0,
            },
            (Self::Bool(obj), Content::Float(other)) => match obj {
                true => *other <= 1.0,
                false => *other <= 0.0,
            },
            (Self::String(obj), Content::String(other)) => obj.as_raw() >= other.as_raw(),
            (Self::Bool(obj), Content::Bool(other)) => obj >= other,
            _ => false,
        }
    }
}

impl PyCmp<Self> for Option<Content<'_, '_>> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (None, None) => true,
            (Some(obj), Some(other)) => obj.eq(other),
            (Some(obj), None) | (None, Some(obj)) => match obj {
                Content::Py(obj) => obj.eq(PyNone::get(obj.py())).unwrap_or(false),
                _ => false,
            },
        }
    }

    fn lt(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(obj), Some(other)) => obj.lt(other),
            _ => false,
        }
    }

    fn gt(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(obj), Some(other)) => obj.gt(other),
            _ => false,
        }
    }

    fn lte(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(obj), Some(other)) => obj.lte(other),
            _ => false,
        }
    }

    fn gte(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(obj), Some(other)) => obj.gte(other),
            _ => false,
        }
    }
}

trait Contains<T> {
    fn contains(&self, other: T) -> Option<bool>;
}

impl Contains<Option<Content<'_, '_>>> for Content<'_, '_> {
    fn contains(&self, other: Option<Content<'_, '_>>) -> Option<bool> {
        match other {
            None => match self {
                Self::Py(obj) => obj.contains(PyNone::get(obj.py())).ok(),
                _ => None,
            },
            Some(Content::Py(other)) => {
                let obj = self.to_py(other.py());
                obj.contains(other).ok()
            }
            Some(Content::String(other)) => match self {
                Self::String(obj) => Some(obj.as_raw().contains(other.as_raw().as_ref())),
                Self::Int(_) | Self::Float(_) | Self::Bool(_) => None,
                Self::Py(obj) => obj.contains(other).ok(),
            },
            Some(Content::Int(n)) => match self {
                Self::Py(obj) => obj.contains(n).ok(),
                _ => None,
            },
            Some(Content::Float(f)) => match self {
                Self::Py(obj) => obj.contains(f).ok(),
                _ => None,
            },
            Some(Content::Bool(b)) => match self {
                Self::Py(obj) => obj.contains(b).ok(),
                _ => None,
            },
        }
    }
}

trait ResolveTuple<'t, 'py> {
    fn resolve(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<(Option<Content<'t, 'py>>, Option<Content<'t, 'py>>), PyRenderError>;
}

impl<'t, 'py> ResolveTuple<'t, 'py> for (IfCondition, IfCondition) {
    fn resolve(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<(Option<Content<'t, 'py>>, Option<Content<'t, 'py>>), PyRenderError> {
        const IGNORE: ResolveFailures = ResolveFailures::IgnoreVariableDoesNotExist;
        Ok(match self {
            (IfCondition::Variable(l), IfCondition::Variable(r)) => {
                let left = l.resolve(py, template, context, IGNORE)?;
                let right = r.resolve(py, template, context, IGNORE)?;
                (left, right)
            }
            (IfCondition::Variable(l), r) => {
                let left = l.resolve(py, template, context, IGNORE)?;
                let right = r
                    .evaluate(py, template, context)
                    .expect("Right cannot be an expression that evaluates to None");
                (left, Some(Content::Bool(right)))
            }
            (l, IfCondition::Variable(r)) => {
                let left = l
                    .evaluate(py, template, context)
                    .expect("Left cannot be an expression that evaluates to None");
                let right = r.resolve(py, template, context, IGNORE)?;
                (Some(Content::Bool(left)), right)
            }
            (l, r) => {
                let left = l
                    .evaluate(py, template, context)
                    .expect("Left cannot be an expression that evaluates to None");
                let right = r
                    .evaluate(py, template, context)
                    .expect("Right cannot be an expression that evaluates to None");
                (Some(Content::Bool(left)), Some(Content::Bool(right)))
            }
        })
    }
}

impl Evaluate for IfCondition {
    #[allow(clippy::too_many_lines)]
    fn evaluate(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
    ) -> Option<bool> {
        Some(match self {
            Self::Variable(v) => v.evaluate(py, template, context)?,
            Self::And(inner) => {
                let left = inner.0.evaluate(py, template, context).unwrap_or(false);
                let right = inner.1.evaluate(py, template, context).unwrap_or(false);
                if left { right } else { false }
            }
            Self::Or(inner) => {
                let left = inner.0.evaluate(py, template, context);
                let right = inner.1.evaluate(py, template, context);
                match left {
                    None => false,
                    Some(left) => {
                        if left {
                            true
                        } else {
                            right.unwrap_or(false)
                        }
                    }
                }
            }
            Self::Not(inner) => match inner.evaluate(py, template, context) {
                None | Some(true) => false,
                Some(false) => true,
            },
            Self::Equal(inner) => match inner.resolve(py, template, context) {
                Ok((l, r)) => l.eq(&r),
                Err(_) => false,
            },
            Self::NotEqual(inner) => match inner.resolve(py, template, context) {
                Ok((l, r)) => l.ne(&r),
                Err(_) => false,
            },
            Self::LessThan(inner) => match inner.resolve(py, template, context) {
                Ok((l, r)) => l.lt(&r),
                Err(_) => false,
            },
            Self::GreaterThan(inner) => match inner.resolve(py, template, context) {
                Ok((l, r)) => l.gt(&r),
                Err(_) => false,
            },
            Self::LessThanEqual(inner) => match inner.resolve(py, template, context) {
                Ok((l, r)) => l.lte(&r),
                Err(_) => false,
            },
            Self::GreaterThanEqual(inner) => match inner.resolve(py, template, context) {
                Ok((l, r)) => l.gte(&r),
                Err(_) => false,
            },
            Self::In(inner) => {
                let Ok(inner) = inner.resolve(py, template, context) else {
                    return Some(false);
                };
                match inner {
                    (l, Some(r)) => r.contains(l).unwrap_or(false),
                    _ => false,
                }
            }
            Self::NotIn(inner) => {
                let Ok(inner) = inner.resolve(py, template, context) else {
                    return Some(false);
                };
                match inner {
                    (l, Some(r)) => !(r.contains(l).unwrap_or(true)),
                    _ => false,
                }
            }
            Self::Is(inner) => {
                let Ok(inner) = inner.resolve(py, template, context) else {
                    return Some(false);
                };
                match inner {
                    (Some(Content::Py(left)), Some(Content::Py(right))) => left.is(&right),
                    (Some(Content::Py(obj)), None) | (None, Some(Content::Py(obj))) => {
                        obj.is(PyNone::get(py).as_any())
                    }
                    (Some(Content::Bool(left)), Some(Content::Py(right))) => {
                        right.is(PyBool::new(py, left).as_any())
                    }
                    (None, None) => true,
                    _ => false,
                }
            }
            Self::IsNot(inner) => {
                let Ok(inner) = inner.resolve(py, template, context) else {
                    return Some(false);
                };
                match inner {
                    (Some(Content::Py(left)), Some(Content::Py(right))) => !left.is(&right),
                    (Some(Content::Bool(left)), Some(Content::Bool(right))) => left != right,
                    (Some(Content::Py(obj)), None) | (None, Some(Content::Py(obj))) => {
                        !obj.is(PyNone::get(py).as_any())
                    }
                    (Some(Content::Bool(left)), Some(Content::Py(right))) => {
                        !right.is(PyBool::new(py, left).as_any())
                    }
                    (Some(Content::Py(left)), Some(Content::Bool(right))) => {
                        !left.is(PyBool::new(py, right).as_any())
                    }
                    (None, None) => false,
                    _ => true,
                }
            }
        })
    }
}

fn render_widthratio(
    args: &[TagElement],
    py: Python<'_>,
    template: TemplateString<'_>,
    context: &mut Context,
) -> Result<String, PyRenderError> {
    let values = args
        .iter()
        .map(|arg| {
            arg.resolve(
                py,
                template,
                context,
                ResolveFailures::IgnoreVariableDoesNotExist,
            )
            .map(|value| value.unwrap_or_else(|| "".as_content()).to_py(py))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let builtins = BUILTINS
        .get_or_try_init(py, || {
            py.import("builtins")
                .map(|module| module.into_any().unbind())
        })?
        .bind(py);
    let width = match builtins.getattr("int")?.call1((&values[2],)) {
        Ok(width) => width,
        Err(error)
            if error.is_instance_of::<PyValueError>(py)
                || error.is_instance_of::<PyTypeError>(py) =>
        {
            return Err(
                TemplateSyntaxError::new_err("widthratio final argument must be a number").into(),
            );
        }
        Err(error) => return Err(error.into()),
    };
    let compute = || -> PyResult<String> {
        let float = builtins.getattr("float")?;
        let value = float.call1((&values[0],))?;
        let maximum = float.call1((&values[1],))?;
        let ratio = value
            .call_method1("__truediv__", (maximum,))?
            .call_method1("__mul__", (width,))?;
        builtins.getattr("round")?.call1((ratio,))?.str()?.extract()
    };
    match compute() {
        Ok(value) => Ok(value),
        Err(error) if error.is_instance_of::<PyZeroDivisionError>(py) => Ok("0".to_string()),
        Err(error)
            if error.is_instance_of::<PyValueError>(py)
                || error.is_instance_of::<PyTypeError>(py)
                || error.is_instance_of::<PyOverflowError>(py) =>
        {
            Ok(String::new())
        }
        Err(error) => Err(error.into()),
    }
}

impl Render for Tag {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        Ok(match self {
            Self::PartialDef { definition, inline } => {
                if *inline {
                    let owner = context
                        .current_template
                        .as_ref()
                        .expect("inside a template")
                        .clone();
                    Cow::Owned(
                        owner
                            .render_body(py, &definition.nodes, context)?
                            .into_owned(),
                    )
                } else {
                    Cow::Borrowed("")
                }
            }
            Self::Partial { name, at } => {
                let owner = context
                    .current_template
                    .as_ref()
                    .expect("inside a template")
                    .clone();
                let definition = owner.native_partials().get(name).ok_or_else(|| {
                    TemplateSyntaxError::new_err(format!(
                        "Partial '{name}' is not defined in the current template."
                    ))
                    .annotate(py, *at, "here", template)
                })?;
                let body = owner.block_template(definition.nodes.clone());
                Cow::Owned(context.render_partial(py, &body)?.into_owned())
            }
            Self::Block { name, nodes } => {
                if let Some(blocks) = &context.python_blocks {
                    Cow::Owned(
                        blocks
                            .bind(py)
                            .get_item(name)?
                            .call_method1("render", (context.django_context(py)?,))?
                            .extract()?,
                    )
                } else {
                    let owner = context
                        .current_template
                        .as_ref()
                        .expect("inside a template")
                        .clone();
                    context.render_block(
                        py,
                        super::inheritance::BlockEntry {
                            name: name.clone(),
                            owner,
                            nodes: nodes.clone(),
                        },
                    )?
                }
            }
            Self::Extends {
                at,
                parent,
                parent_token,
                relative,
                ..
            } => {
                let parent_value = if let Some(relative) = relative {
                    PyString::new(py, relative).into_any()
                } else {
                    parent
                        .resolve(py, template, context, ResolveFailures::Raise)?
                        .map_or_else(|| PyString::new(py, "").into_any(), |value| value.to_py(py))
                };
                context
                    .render_parent(
                        py,
                        parent_value,
                        parent_token,
                        !matches!(parent, TagElement::Text(_)),
                    )
                    .map_err(|error| match error {
                        PyRenderError::PyErr(error) => {
                            error.annotate(py, *at, "here", template).into()
                        }
                        error => error,
                    })?
            }
            Self::Regroup(group) => {
                let results = PyList::empty(py);
                if let Some(target) = group.target.resolve(
                    py,
                    template,
                    context,
                    ResolveFailures::IgnoreVariableDoesNotExist,
                )? {
                    let target = target.to_py(py);
                    if !target.is_none() {
                        let python_context = context.django_context(py)?;
                        let kwargs = PyDict::new(py);
                        kwargs.set_item("ignore_failures", true)?;
                        let grouped_result = py
                            .import("django.template.defaulttags")?
                            .getattr("GroupedResult")?;
                        let mut previous: Option<Bound<'_, PyAny>> = None;
                        let mut items = PyList::empty(py);
                        for item in target.try_iter()? {
                            let item = item?;
                            python_context.set_item(&group.asvar, &item)?;
                            let key = group.expression.bind(py).call_method(
                                "resolve",
                                (&python_context,),
                                Some(&kwargs),
                            )?;
                            let same = match &previous {
                                Some(previous) => key.is(previous) || key.eq(previous)?,
                                None => false,
                            };
                            if !same {
                                items = PyList::empty(py);
                                results.append(grouped_result.call1((&key, &items))?)?;
                                previous = Some(key);
                            }
                            items.append(item)?;
                        }
                    }
                }
                context.insert(group.asvar.clone(), results.into_any())?;
                Cow::Borrowed("")
            }
            Self::IfChanged {
                id,
                values,
                truthy,
                falsey,
            } => {
                let mut output = None;
                let comparison = if values.is_empty() {
                    let rendered = truthy.render(py, template, context)?.into_owned();
                    let value = PyString::new(py, &rendered).into_any();
                    output = Some(rendered);
                    value
                } else {
                    let list = PyList::empty(py);
                    for value in values {
                        let value = value.resolve(
                            py,
                            template,
                            context,
                            ResolveFailures::IgnoreVariableDoesNotExist,
                        )?;
                        list.append(
                            value.map_or_else(|| py.None().into_bound(py), |value| value.to_py(py)),
                        )?;
                    }
                    list.into_any()
                };
                if context.has_changed(py, *id, comparison)? {
                    match output {
                        Some(output) if !output.is_empty() => Cow::Owned(output),
                        _ => truthy.render(py, template, context)?,
                    }
                } else {
                    falsey.render(py, template, context)?
                }
            }
            Self::ResetCycle(id) => {
                context.reset_cycle(py, *id)?;
                Cow::Borrowed("")
            }
            Self::With { assignments, nodes } => {
                let values = assignments
                    .iter()
                    .map(|(name, value)| {
                        value
                            .resolve(py, template, context, ResolveFailures::Raise)
                            .map(|value| (name, value.unwrap_or_else(|| "".as_content()).to_py(py)))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut scoped = context.clone_ref(py);
                let scope = if let Some(live) = &scoped.formatting_context {
                    Some(live.bind(py).call_method0("push")?)
                } else {
                    None
                };
                let local_names = values
                    .iter()
                    .map(|(name, _)| (*name).clone())
                    .collect::<Vec<_>>();
                for (name, value) in values {
                    scoped.insert(name.clone(), value)?;
                }
                let rendered = nodes.render(py, template, &mut scoped);
                if let Some(scope) = scope {
                    scope.call_method1("__exit__", (py.None(), py.None(), py.None()))?;
                }
                context.merge_render_state(py, scoped, &local_names)?;
                rendered?
            }
            Self::FilterBlock { filters, nodes } => {
                let output = nodes.render(py, template, context)?;
                let safe = MARK_BLOCK_SAFE
                    .import(py, "django.utils.safestring", "mark_safe")?
                    .call1((output.as_ref(),))?;
                let mut value = Some(Content::Py(safe));
                for filter in filters {
                    value = filter.resolve(value, py, template, context)?;
                }
                match value {
                    Some(value) => Cow::Owned(value.to_py(py).str()?.extract::<String>()?),
                    None => Cow::Borrowed(""),
                }
            }
            Self::Spaceless(nodes) => {
                let output = nodes.render(py, template, context)?;
                Cow::Owned(
                    STRIP_SPACES
                        .import(py, "django.utils.html", "strip_spaces_between_tags")?
                        .call1((output.trim(),))?
                        .extract()?,
                )
            }
            Self::Verbatim(nodes) => nodes.render(py, template, context)?,
            Self::Debug => {
                let values = context.as_dict(py)?;
                for name in ["True", "False", "None"] {
                    if values.contains(name)? {
                        values.del_item(name)?;
                    }
                }
                let ctx = py
                    .import("django.template.context")?
                    .getattr("Context")?
                    .call1((values,))?;
                Cow::Owned(
                    DEBUG_NODE
                        .import(py, "django.template.defaulttags", "DebugNode")?
                        .call0()?
                        .call_method1("render", (ctx,))?
                        .extract()?,
                )
            }
            Self::WidthRatio { args, asvar } => {
                let result = render_widthratio(args, py, template, context)?;
                if let Some(name) = asvar {
                    context.insert(name.clone(), PyString::new(py, &result).into_any())?;
                    Cow::Borrowed("")
                } else {
                    Cow::Owned(result)
                }
            }
            Self::Cycle(cycle) => cycle.render(py, template, context)?,
            Self::Autoescape { enabled, nodes } => {
                let autoescape = context.autoescape;
                context.autoescape = enabled.into();
                if let Some(live) = &context.formatting_context {
                    live.bind(py).setattr("autoescape", context.autoescape)?;
                }
                let rendered = nodes
                    .render(py, template, context)
                    .map(|output| Cow::Owned(output.into_owned()));
                context.autoescape = autoescape;
                if let Some(live) = &context.formatting_context {
                    live.bind(py).setattr("autoescape", autoescape)?;
                }
                rendered?
            }
            Self::If {
                condition,
                truthy,
                falsey,
            } => {
                if condition.evaluate(py, template, context).unwrap_or(false) {
                    truthy.render(py, template, context)?
                } else {
                    falsey.render(py, template, context)?
                }
            }
            Self::For(for_tag) => for_tag.render(py, template, context)?,
            Self::Include(include_tag) => include_tag.render(py, template, context)?,
            Self::Load => Cow::Borrowed(""),
            Self::PythonNode(node) => {
                let live = context.formatting_context.as_ref().ok_or_else(|| {
                    pyo3::exceptions::PyRuntimeError::new_err(
                        "Interop regions require a live Django Context",
                    )
                })?;
                let output = node
                    .0
                    .bind(py)
                    .call_method1("render_annotated", (live.bind(py),))
                    .inspect_err(|error| {
                        // Native and Django Origin objects differ, so Django's
                        // annotation hook may leave metadata to the owner.
                        let culprit = error
                            .value(py)
                            .getattr("_culprit_node")
                            .unwrap_or_else(|_| node.0.bind(py).clone());
                        if let Ok(token) = culprit.getattr("token") {
                            let _ = error.value(py).setattr("token", token);
                        }
                    })?;
                context.refresh_formatting_flags(py)?;
                Cow::Owned(output.extract::<String>()?)
            }
            Self::QueryString(tag) => {
                let mut args = build_args(py, template, context, &tag.args)?;
                let kwargs = build_kwargs(py, template, context, &tag.kwargs)?;
                let python_context = context.django_context(py)?;
                if let Some(request) = &context.request {
                    python_context.setattr("request", request.bind(py))?;
                }
                args.push_front(python_context);
                let output = tag
                    .func
                    .bind(py)
                    .call(PyTuple::new(py, args)?, Some(&kwargs))?;
                if let Some(target) = &tag.target_var {
                    context.insert(target.clone(), output)?;
                    Cow::Borrowed("")
                } else {
                    Content::Py(output).render(context)?
                }
            }
            Self::InclusionTag(tag) => tag.render(py, template, context)?,
            Self::SimpleTag(simple_tag) => simple_tag.render(py, template, context)?,
            Self::SimpleBlockTag(simple_tag) => simple_tag.render(py, template, context)?,
            Self::Url(url) => url.render(py, template, context)?,
            Self::CsrfToken(csrf_token) => csrf_token.render(py, template, context)?,
            Self::Lorem(lorem) => lorem.render(py, template, context)?,
            Self::Comment(_) => Cow::Borrowed(""),
            Self::Now(now) => now.render(py, template, context)?,
            Self::FirstOf(firstof) => firstof.render(py, template, context)?,
            Self::TemplateTag(template_tag) => Cow::Borrowed(template_tag.output()),
        })
    }
}

impl For {
    fn render_python<'t>(
        &self,
        iterable: &Bound<'_, PyAny>,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let mut list: Vec<_> = match iterable.try_iter() {
            Ok(iterator) => iterator.collect(),
            Err(error) => {
                let error = error.annotate(py, self.iterable.at, "here", template);
                return Err(error.into());
            }
        };
        if list.is_empty() {
            return self.empty.render(py, template, context);
        }
        if self.reversed {
            list.reverse();
        }
        context.push_for_loop(list.len());
        let mut rendered = String::new();
        for (index, values) in list.into_iter().enumerate() {
            let values = match values {
                Ok(values) => values,
                Err(error) => {
                    let error =
                        error.annotate(py, self.iterable.at, "while iterating this", template);
                    return Err(error.into());
                }
            };
            context.push_variables(
                &self.variables.names,
                self.variables.at,
                values,
                self.iterable.at,
                index,
                template,
            )?;
            context.sync_loop_context(py, index == 0)?;
            rendered.push_str(&self.body.render(py, template, context)?);
            context.increment_for_loop();
        }
        context.pop_variables();
        context.pop_for_loop();
        Ok(Cow::Owned(rendered))
    }

    fn render_string<'t>(
        &self,
        string: &str,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        if string.is_empty() {
            return self.empty.render(py, template, context);
        }
        if self.variables.names.len() > 1 {
            return Err(RenderError::TupleUnpackError {
                expected_count: self.variables.names.len(),
                actual_count: 1,
                expected_at: self.variables.at.into(),
                actual_at: self.iterable.at.into(),
            }
            .into());
        }
        let mut rendered = String::new();
        let mut chars: Vec<_> = string.chars().collect();
        if self.reversed {
            chars.reverse();
        }

        let variable = &self.variables.names[0];
        context.push_for_loop(chars.len());
        for (index, c) in chars.into_iter().enumerate() {
            let c = PyString::new(py, &c.to_string());
            context.push_variable(variable.clone(), c.into_any(), index);
            context.sync_loop_context(py, index == 0)?;
            rendered.push_str(&self.body.render(py, template, context)?);
            context.increment_for_loop();
        }
        context.pop_variables();
        context.pop_for_loop();
        Ok(Cow::Owned(rendered))
    }
}

impl Render for For {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let scope = if let Some(django_context) = &context.formatting_context {
            let django_context = django_context.bind(py);
            let scope = django_context.call_method0("push")?;
            Some(scope)
        } else {
            None
        };
        let result = self.render_loop(py, template, context);
        if let Some(scope) = scope {
            scope.call_method1("__exit__", (py.None(), py.None(), py.None()))?;
        }
        result
    }
}

impl For {
    fn render_loop<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let Some(iterable) = self.iterable.iterable.resolve(
            py,
            template,
            context,
            ResolveFailures::IgnoreVariableDoesNotExist,
        )?
        else {
            return self.empty.render(py, template, context);
        };
        match iterable {
            Content::Py(iterable) => self.render_python(&iterable, py, template, context),
            Content::String(s) => self.render_string(s.as_raw(), py, template, context),
            value @ (Content::Float(_) | Content::Int(_) | Content::Bool(_)) => {
                self.render_python(&value.to_py(py), py, template, context)
            }
        }
    }
}

enum IncludeTemplate<'py> {
    Template(Arc<Template>),
    Callable(Bound<'py, PyAny>),
}

impl<'t, 'py> IncludeTemplate<'py> {
    fn render(
        &'t self,
        py: Python<'py>,
        context: &mut Context,
        at: At,
        template: TemplateString<'t>,
    ) -> RenderResult<'t> {
        match self {
            Self::Template(template) => template.render(py, context),
            Self::Callable(callable) => {
                let py_context = build_pycontext(py, context)?;
                let result = callable.call1((py_context.clone(),));
                retrieve_context(py, py_context, context);
                match result {
                    Ok(content) => Ok(Cow::Owned(content.to_string())),
                    Err(error) => Err(error.annotate(py, at, "here", template).into()),
                }
            }
        }
    }
}

impl Include {
    fn template_at(&self) -> At {
        match &self.template_name {
            IncludeTemplateName::Text(text) => text.at,
            IncludeTemplateName::Variable(TagElement::Variable(variable)) => variable.at,
            IncludeTemplateName::Variable(TagElement::Filter(filter)) => filter.all_at,
            IncludeTemplateName::Variable(TagElement::ForVariable(variable)) => variable.at,
            IncludeTemplateName::Relative(relative) => relative.at,
            IncludeTemplateName::Variable(_) => unreachable!(),
        }
    }

    fn invalid_template_name(
        &self,
        py: Python<'_>,
        content: &str,
        template: TemplateString<'_>,
    ) -> PyRenderError {
        PyTypeError::new_err("Included template name must be a string or iterable of strings.")
            .annotate(
                py,
                self.template_at(),
                &format!("invalid template name: {content}"),
                template,
            )
            .into()
    }

    fn resolve_template_name<'t, 'py>(
        &'t self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<Content<'t, 'py>, PyRenderError> {
        let template_name = match &self.template_name {
            IncludeTemplateName::Text(text) => text
                .resolve(py, template, context, ResolveFailures::Raise)
                .expect("Text should always be resolvable"),
            IncludeTemplateName::Variable(tag_element) => {
                tag_element.resolve(py, template, context, ResolveFailures::Raise)?
            }
            IncludeTemplateName::Relative(relative) => Some(Content::String(
                ContentString::String(Cow::Borrowed(&relative.path)),
            )),
        };
        match template_name {
            Some(template_name) => Ok(template_name),
            None => {
                let error = TemplateDoesNotExist::new_err("No template names provided").annotate(
                    py,
                    self.template_at(),
                    "This variable is not in the context",
                    template,
                );
                Err(error.into())
            }
        }
    }

    fn get_template<'t, 'py>(
        &self,
        template_name: Content<'t, 'py>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &'t mut Context,
    ) -> Result<IncludeTemplate<'py>, PyRenderError> {
        match template_name {
            Content::String(content) => {
                let key = IncludeTemplateKey::String(content.content().to_string());
                context
                    .get_or_insert_include(py, &self.engine, &key, self.id)
                    .map(IncludeTemplate::Template)
            }
            Content::Py(content) => {
                if !content.is_truthy()? {
                    return Err(TemplateDoesNotExist::new_err("No template names provided")
                        .annotate(
                            py,
                            self.template_at(),
                            "This variable is not in the context",
                            template,
                        )
                        .into());
                }
                if let Some(render) = content.getattr_opt(intern!(py, "render"))?
                    && render.is_callable()
                {
                    Ok(IncludeTemplate::Callable(render))
                } else if content.is_instance_of::<PyString>() {
                    let template_path = content
                        .extract()
                        .expect("PyString should be compatible with Cow<str>");
                    let template_path = match construct_relative_path(
                        template_path,
                        self.origin.as_deref(),
                        self.template_at(),
                    )
                    .map_err(RenderError::from)?
                    {
                        Some(path) => path.to_string(),
                        None => template_path.to_string(),
                    };
                    let key = IncludeTemplateKey::String(template_path);
                    context
                        .get_or_insert_include(py, &self.engine, &key, self.id)
                        .map(IncludeTemplate::Template)
                } else {
                    let promise = PROMISE.import(py, "django.utils.functional", "Promise")?;
                    if content.is_instance(promise)? {
                        return Err(PyTypeError::new_err(
                            "Included template name cannot be a translatable string.",
                        )
                        .annotate(
                            py,
                            self.template_at(),
                            &format!("invalid template name: {content:?}"),
                            template,
                        )
                        .into());
                    }
                    let Ok(templates) = content.extract::<Vec<String>>() else {
                        return Err(self.invalid_template_name(
                            py,
                            &format!("{content}"),
                            template,
                        ));
                    };
                    let key = IncludeTemplateKey::Vec(templates);
                    context
                        .get_or_insert_include(py, &self.engine, &key, self.id)
                        .map(IncludeTemplate::Template)
                }
            }
            Content::Int(content) => {
                return Err(self.invalid_template_name(py, &format!("{content}"), template));
            }
            Content::Float(content) => {
                return Err(self.invalid_template_name(py, &format!("{content}"), template));
            }
            Content::Bool(true) => return Err(self.invalid_template_name(py, "True", template)),
            Content::Bool(false) => return Err(self.invalid_template_name(py, "False", template)),
        }
        .map_err(|error| {
            error
                .annotate(py, self.template_at(), "here", template)
                .into()
        })
    }
}

impl Render for Include {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let template_name = self.resolve_template_name(py, template, context)?;
        let include = self.get_template(template_name, py, template, context)?;
        match self.only {
            false => {
                let mut names = Vec::new();
                let mut values = Vec::new();
                for (at, element) in &self.kwargs {
                    let key = template.content(*at);
                    names.push(key);

                    match element.resolve(
                        py,
                        template,
                        context,
                        ResolveFailures::IgnoreVariableDoesNotExist,
                    )? {
                        Some(value) => values.push(value.to_py(py)),
                        None => values.push(PyString::new(py, "").into_any()),
                    }
                }
                let scope = if let Some(live) = &context.formatting_context {
                    Some(live.bind(py).call_method0("push")?)
                } else {
                    None
                };
                for (key, value) in names.iter().zip(values) {
                    if let Some(live) = &context.formatting_context {
                        live.bind(py).set_item(key, &value)?;
                    }
                    context.append(key.to_string(), value);
                }
                let rendered = include
                    .render(py, context, self.template_at(), template)
                    .map(|content| Cow::Owned(content.into_owned()));
                for key in names {
                    context.pop_variable(key);
                }
                if let Some(scope) = scope {
                    scope.call_method1("__exit__", (py.None(), py.None(), py.None()))?;
                }
                rendered
            }
            true => {
                let mut inner_context = HashMap::new();
                for (at, element) in &self.kwargs {
                    let key = template.content(*at).to_string();
                    if let Some(value) = element.resolve(
                        py,
                        template,
                        context,
                        ResolveFailures::IgnoreVariableDoesNotExist,
                    )? {
                        inner_context.insert(key, value.to_py(py).unbind());
                    }
                }
                let request = context
                    .request
                    .as_ref()
                    .map(|request| request.clone_ref(py));
                let mut new_context = Context::new(inner_context, request, context.autoescape);
                new_context.use_l10n = context.use_l10n;
                new_context.use_tz = context.use_tz;
                new_context.format_values = context.format_values;
                include
                    .render(py, &mut new_context, self.template_at(), template)
                    .map(|content| Cow::Owned(content.into_owned()))
            }
        }
    }
}

fn call_tag<'t, 'py>(
    py: Python<'py>,
    func: &Arc<Py<PyAny>>,
    at: At,
    template: TemplateString<'t>,
    args: VecDeque<Bound<'py, PyAny>>,
    kwargs: Bound<'py, PyDict>,
) -> Result<Bound<'py, PyAny>, PyRenderError> {
    let func = func.bind(py);
    match func.call(
        PyTuple::new(py, args).expect("All arguments should be valid Python objects"),
        Some(&kwargs),
    ) {
        Ok(content) => Ok(content),
        Err(error) => Err(error.annotate(py, at, "here", template).into()),
    }
}

fn build_pycontext<'py>(py: Python<'py>, context: &mut Context) -> PyResult<Bound<'py, PyAny>> {
    if let Some(live) = &context.formatting_context {
        return Ok(live.bind(py).clone());
    }
    // Take ownership of `context` so we can pass it to Python.
    // The `context` variable now points to an empty `Context` instance which will not be
    // used except as a placeholder.
    let swapped_context = std::mem::take(context);

    // Wrap the context as a Python object
    Ok(Bound::new(py, PyContext::new(swapped_context))?.into_any())
}

fn retrieve_context<'py>(py: Python<'py>, py_context: Bound<'py, PyAny>, context: &mut Context) {
    if context.formatting_context.is_some() {
        return;
    }
    // Retrieve the PyContext wrapper from Python
    let extracted_context: PyContext = py_context
        .extract()
        .expect("The type of py_context should not have changed");
    // Ensure we only hold one reference in Rust by dropping the Python object.
    drop(py_context);

    // Try to remove the Context from the PyContext
    let inner_context = match Arc::try_unwrap(extracted_context.context) {
        // Fast path when we have the only reference in the Arc.
        Ok(inner_context) => inner_context
            .into_inner()
            .expect("Mutex should be unlocked because Arc refcount is one."),
        // Slow path when Python has held on to the context for some reason.
        // We can still do the right thing by cloning.
        Err(inner_context) => {
            let guard = inner_context
                .lock_py_attached(py)
                .expect("Mutex should not be poisoned");
            guard.clone_ref(py)
        }
    };
    // Put the Context back in `context`
    let _ = std::mem::replace(context, inner_context);
}

fn build_arg<'py>(
    py: Python<'py>,
    template: TemplateString<'_>,
    context: &mut Context,
    arg: &TagElement,
) -> Result<Bound<'py, PyAny>, PyRenderError> {
    let arg = match arg.resolve(py, template, context, ResolveFailures::Raise)? {
        Some(arg) => arg.to_py(py),
        None => PyString::intern(py, "").into_any(),
    };
    Ok(arg)
}

fn build_args<'py>(
    py: Python<'py>,
    template: TemplateString<'_>,
    context: &mut Context,
    args: &[TagElement],
) -> Result<VecDeque<Bound<'py, PyAny>>, PyRenderError> {
    args.iter()
        .map(|arg| build_arg(py, template, context, arg))
        .collect()
}

fn build_kwargs<'py>(
    py: Python<'py>,
    template: TemplateString<'_>,
    context: &mut Context,
    kwargs: &Vec<(String, TagElement)>,
) -> Result<Bound<'py, PyDict>, PyRenderError> {
    let py_kwargs = PyDict::new(py);
    for (key, value) in kwargs {
        let value = value.resolve(py, template, context, ResolveFailures::Raise)?;
        py_kwargs.set_item(key, value)?;
    }
    Ok(py_kwargs)
}

fn store_target_var<'t>(
    _py: Python<'_>,
    context: &mut Context,
    content: Bound<'_, PyAny>,
    target_var: Option<&String>,
) -> RenderResult<'t> {
    match target_var {
        None => {
            let output = if context.autoescape {
                content
                    .py()
                    .import("django.utils.html")?
                    .getattr("conditional_escape")?
                    .call1((content,))?
            } else {
                content
            };
            Ok(Cow::Owned(output.str()?.extract::<String>()?))
        }
        Some(target_var) => {
            context.insert(target_var.clone(), content)?;
            Ok(Cow::Borrowed(""))
        }
    }
}

impl Render for InclusionTag {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let mut args = build_args(py, template, context, &self.call.args)?;
        let kwargs = build_kwargs(py, template, context, &self.call.kwargs)?;
        if self.call.takes_context {
            let python_context = context.django_context(py)?;
            if let Some(request) = &context.request {
                python_context.setattr("request", request.bind(py))?;
            }
            args.push_front(python_context);
        }
        let values = self
            .call
            .func
            .bind(py)
            .call(PyTuple::new(py, args)?, Some(&kwargs))?;
        let mut values: HashMap<String, Py<PyAny>> = values.extract()?;
        if let Some(csrf) = context.get("csrf_token")
            && !csrf.is_none(py)
        {
            values.insert("csrf_token".to_string(), csrf.clone_ref(py));
        }
        // Context.new() restores builtins and preserves formatting flags, while
        // excluding the caller's variables, loop state and context processors.
        for (name, value) in [
            ("True", PyBool::new(py, true).to_owned().into_any()),
            ("False", PyBool::new(py, false).to_owned().into_any()),
            ("None", py.None().into_bound(py)),
        ] {
            values
                .entry(name.to_string())
                .or_insert_with(|| value.unbind());
        }
        let mut child = Context::new(
            values,
            context
                .request
                .as_ref()
                .map(|request| request.clone_ref(py)),
            context.autoescape,
        );
        child.use_l10n = context.use_l10n;
        child.use_tz = context.use_tz;
        child.format_values = context.format_values;
        let filename = self.filename.bind(py);
        let filename = filename
            .getattr("template")
            .unwrap_or_else(|_| filename.clone());
        if let Ok(loaded) = filename.extract::<PyRef<'_, Template>>() {
            return loaded
                .render(py, &mut child)
                .map(|output| Cow::Owned(output.into_owned()));
        }
        if filename.hasattr("render")? {
            return Ok(Cow::Owned(
                filename
                    .call_method1("render", (child.django_context(py)?,))?
                    .extract()?,
            ));
        }
        let key = if let Ok(name) = filename.extract::<String>() {
            IncludeTemplateKey::String(name)
        } else {
            IncludeTemplateKey::Vec(filename.extract()?)
        };
        let loaded = context.get_or_insert_include(py, &self.engine, &key, self.id)?;
        loaded
            .render(py, &mut child)
            .map(|output| Cow::Owned(output.into_owned()))
    }
}

impl Render for SimpleTag {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let mut args = build_args(py, template, context, &self.args)?;
        let kwargs = build_kwargs(py, template, context, &self.kwargs)?;
        let content = if self.takes_context {
            let py_context = build_pycontext(py, context)?;
            args.push_front(py_context.clone());

            // Actually call the tag
            let result = call_tag(py, &self.func, self.at, template, args, kwargs);

            retrieve_context(py, py_context, context);

            // Return the result of calling the tag
            result?
        } else {
            call_tag(py, &self.func, self.at, template, args, kwargs)?
        };
        store_target_var(py, context, content, self.target_var.as_ref())
    }
}

impl Render for SimpleBlockTag {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let mut args = build_args(py, template, context, &self.args)?;
        let kwargs = build_kwargs(py, template, context, &self.kwargs)?;

        let content = self.nodes.render(py, template, context)?;
        let content = py
            .import("django.utils.safestring")?
            .getattr("mark_safe")?
            .call1((&*content,))?;
        args.push_front(content);

        let content = if self.takes_context {
            let py_context = build_pycontext(py, context)?;
            args.push_front(py_context.clone());

            // Actually call the tag
            let result = call_tag(py, &self.func, self.at, template, args, kwargs);

            retrieve_context(py, py_context, context);

            // Return the result of calling the tag
            result?
        } else {
            call_tag(py, &self.func, self.at, template, args, kwargs)?
        };
        store_target_var(py, context, content, self.target_var.as_ref())
    }
}

impl Render for Now {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        if let Some(expression) = &self.expression {
            let format = expression
                .resolve(
                    py,
                    template,
                    context,
                    ResolveFailures::IgnoreVariableDoesNotExist,
                )?
                .unwrap_or_else(|| "".as_content())
                .to_py(py);
            let result = py
                .import("django_rusty_templates.diagnostics")?
                .getattr("format_now")?
                .call1((format, self.escape && context.autoescape))?;
            if let Some(at) = self.asvar {
                context.insert(template.content(at).to_string(), result)?;
                return Ok(Cow::Borrowed(""));
            }
            return Ok(Cow::Owned(
                result
                    .cast_into::<PyString>()
                    .map_err(PyErr::from)?
                    .to_str()?
                    .to_owned(),
            ));
        }
        let tz_mod = DJANGO_TIMEZONE
            .get_or_try_init(py, || -> Result<Py<PyAny>, PyRenderError> {
                Ok(py.import("django.utils.timezone")?.into())
            })?
            .bind(py);
        let now_dt = tz_mod.call_method0("now")?;

        let is_named_format = matches!(
            self.format.as_str(),
            "DATE_FORMAT"
                | "DATETIME_FORMAT"
                | "SHORT_DATE_FORMAT"
                | "SHORT_DATETIME_FORMAT"
                | "YEAR_MONTH_FORMAT"
                | "MONTH_DAY_FORMAT"
                | "TIME_FORMAT"
        );

        let use_named_logic = self.format.is_empty() || is_named_format;

        let result: Bound<'_, PyAny> = if use_named_logic {
            let fmt_mod = DJANGO_FORMATS
                .get_or_try_init(py, || -> Result<Py<PyAny>, PyRenderError> {
                    Ok(py.import("django.utils.formats")?.into())
                })?
                .bind(py);
            fmt_mod.call_method1("date_format", (now_dt, &self.format))?
        } else {
            let df_mod = DJANGO_DATEFORMAT
                .get_or_try_init(py, || -> Result<Py<PyAny>, PyRenderError> {
                    Ok(py.import("django.utils.dateformat")?.into())
                })?
                .bind(py);
            df_mod.call_method1("format", (now_dt, &self.format))?
        };

        if let Some(asvar_at) = self.asvar {
            let var_name = template.content(asvar_at);
            context.insert(var_name.to_string(), result)?;
            Ok(Cow::Borrowed(""))
        } else {
            let rendered = result.cast_into::<PyString>().map_err(PyErr::from)?;
            Ok(Cow::Owned(rendered.to_str()?.to_owned()))
        }
    }
}

impl CsrfToken {
    const MISSING_WARNING: &str = "A {% csrf_token %} was used in a template, but the context did not provide the value.  This is usually caused by not providing a request.";

    fn input_html(token_str: &str) -> String {
        format!(
            r#"<input type="hidden" name="csrfmiddlewaretoken" value="{}">"#,
            html_escape::encode_quoted_attribute(token_str)
        )
    }
}

impl Render for CsrfToken {
    fn render<'t>(
        &self,
        py: Python<'_>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        match context.get("csrf_token") {
            Some(token) => {
                let bound_token = token.bind(py);
                if let Ok(token_str) = bound_token.extract::<String>() {
                    if token_str.is_empty() || token_str == "NOTPROVIDED" {
                        Ok(Cow::Borrowed(""))
                    } else {
                        Ok(Cow::Owned(Self::input_html(&token_str)))
                    }
                } else if bound_token.is_truthy()? {
                    let token_py_str = bound_token.str()?;
                    let token_str = token_py_str.to_str()?;
                    Ok(Cow::Owned(Self::input_html(token_str)))
                } else {
                    Ok(Cow::Borrowed(""))
                }
            }
            None => {
                let settings = DJANGO_SETTINGS.import(py, "django.conf", "settings")?;
                let debug = settings.getattr("DEBUG")?.is_truthy()?;

                if debug {
                    let warn = WARNINGS_WARN.import(py, "warnings", "warn")?;
                    warn.call1((Self::MISSING_WARNING,))?;
                }

                Ok(Cow::Borrowed(""))
            }
        }
    }
}

impl Render for Lorem {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let count_content = self.count.resolve(
            py,
            template,
            context,
            ResolveFailures::IgnoreVariableDoesNotExist,
        )?;
        let val = count_content
            .and_then(|c| c.to_bigint())
            .and_then(|n| n.to_i64())
            .unwrap_or(1);

        let text = match self.method {
            LoremMethod::Words => {
                let final_count = match (val, self.common) {
                    (val, true) if val < 0 => (COMMON_WORDS.len() as i64 + val).max(0) as usize,
                    (val, false) if val < 0 => 0,
                    (val, _) => val as usize,
                };
                words(final_count, self.common)
            }
            LoremMethod::Paragraphs | LoremMethod::Blocks => {
                if val <= 0 {
                    return Ok(Cow::Borrowed(""));
                } else {
                    let count = val as usize;
                    let paras = paragraphs(count, self.common);
                    if matches!(self.method, LoremMethod::Paragraphs) {
                        paras
                            .into_iter()
                            .map(|p| format!("<p>{}</p>", p))
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    } else {
                        paras.join("\n\n")
                    }
                }
            }
        };

        Ok(Cow::Owned(text))
    }
}

impl Render for FirstOf {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        for var in &self.vars {
            if let Some(content) = var.resolve(
                py,
                template,
                context,
                ResolveFailures::IgnoreVariableDoesNotExist,
            )? && content.to_bool().unwrap_or(false)
            {
                if let Some(asvar) = &self.asvar {
                    let rendered = RENDER_FIRSTOF_VALUE
                        .import(py, "django.template.base", "render_value_in_context")?
                        .call1((content.to_py(py), context.django_context(py)?))?;
                    context.insert(asvar.to_string(), rendered)?;
                    return Ok(Cow::Borrowed(""));
                }

                let format = content.render(context)?.to_string();
                return Ok(Cow::Owned(format));
            }
        }
        if let Some(asvar) = &self.asvar {
            context.insert(asvar.to_string(), PyString::new(py, "").into_any())?;
        }
        Ok(Cow::Borrowed(""))
    }
}

struct ResolvedCycleValue<'t, 'py> {
    at: At,
    content: Option<Content<'t, 'py>>,
}

impl SimpleCycle {
    fn resolve_next<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<ResolvedCycleValue<'t, 'py>, PyRenderError> {
        let index = context.next_cycle_index(py, self.id, self.values.len())?;
        let CycleValue { value, at } = &self.values[index];
        let content = value.resolve(py, template, context, ResolveFailures::Raise)?;
        Ok(ResolvedCycleValue { at: *at, content })
    }
}

impl NamedCycle {
    fn resolve_and_store<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<ResolvedCycleValue<'t, 'py>, PyRenderError> {
        let ResolvedCycleValue { at, content } = self.cycle.resolve_next(py, template, context)?;

        if let Some(content) = &content {
            context.insert_cycle(self.asvar.clone(), content.to_py(py))?;
        } else {
            context.insert_cycle(self.asvar.clone(), intern!(py, "").clone().into_any())?;
        }

        Ok(ResolvedCycleValue { at, content })
    }
}

impl Render for SimpleCycle {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let ResolvedCycleValue { at, content } = self.resolve_next(py, template, context)?;
        let Some(content) = content else {
            return Ok(Cow::Borrowed(""));
        };

        Ok(content
            .render(context)
            .map_err(|error| error.annotate(py, at, "here", template))?)
    }
}

impl Render for NamedCycle {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let ResolvedCycleValue { at, content } = self.resolve_and_store(py, template, context)?;
        let Some(content) = content else {
            return Ok(Cow::Borrowed(""));
        };

        Ok(content
            .render(context)
            .map_err(|error| error.annotate(py, at, "here", template))?)
    }
}

impl Render for SilentNamedCycle {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        self.cycle.resolve_and_store(py, template, context)?;
        Ok(Cow::Borrowed(""))
    }
}

impl Render for Cycle {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        match self {
            Self::Simple(cycle) => cycle.render(py, template, context),
            Self::Named(cycle) => cycle.render(py, template, context),
            Self::SilentNamed(cycle) => cycle.render(py, template, context),
        }
    }
}
