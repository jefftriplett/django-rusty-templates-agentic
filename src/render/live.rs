//! Reads and writes on a live Django `Context` without its Python methods.
//!
//! `BaseContext.__getitem__` walks `reversed(self.dicts)` with `key in d`
//! and `d[key]`, and `__setitem__` writes `self.dicts[-1][key]`. For a context
//! whose type keeps those methods, the same walk is done here, reading plain
//! `dict` and `ContextDict` layers directly. Other layers and overridden
//! methods use Python.

use std::cell::Cell;

use pyo3::exceptions::PyKeyError;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyDict, PyList, PyString};

static BASE_CONTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static CONTEXT_DICT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static CONTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static REQUEST_CONTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Which of `BaseContext`'s `__getitem__` and `__setitem__` a context's type
/// keeps, remembered for one context object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Methods {
    key: (usize, usize),
    get: bool,
    set: bool,
}

impl Methods {
    fn key(context: &Bound<'_, PyAny>) -> (usize, usize) {
        (
            context.as_ptr() as usize,
            context.get_type().as_ptr() as usize,
        )
    }

    fn read(context: &Bound<'_, PyAny>) -> PyResult<Self> {
        let py = context.py();
        let base = BASE_CONTEXT.import(py, "django.template.context", "BaseContext")?;
        let kind = context.get_type();
        let same = |name: &Bound<'_, PyString>| -> PyResult<bool> {
            Ok(kind.getattr(name)?.is(base.getattr(name)?))
        };
        Ok(Self {
            key: Self::key(context),
            get: same(intern!(py, "__getitem__"))?,
            set: same(intern!(py, "__setitem__"))?,
        })
    }

    /// The methods of `context`, read once per context object and type.
    pub fn cached(cache: &Cell<Option<Self>>, context: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Some(methods) = cache.get()
            && methods.key == Self::key(context)
        {
            return Ok(methods);
        }
        let methods = Self::read(context)?;
        cache.set(Some(methods));
        Ok(methods)
    }
}

fn dicts<'py>(context: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyList>>> {
    Ok(context
        .getattr(intern!(context.py(), "dicts"))?
        .cast_into_exact::<PyList>()
        .ok())
}

/// A layer that is a `dict` or `ContextDict`, neither of which overrides
/// lookups.
fn plain_layer<'a, 'py>(layer: &'a Bound<'py, PyAny>) -> PyResult<Option<&'a Bound<'py, PyDict>>> {
    let py = layer.py();
    if layer.is_exact_instance_of::<PyDict>()
        || layer
            .get_type()
            .is(CONTEXT_DICT.import(py, "django.template.context", "ContextDict")?)
    {
        return Ok(Some(layer.cast::<PyDict>()?));
    }
    Ok(None)
}

/// `context[key]`, or `None` for a `KeyError`.
pub fn get<'py>(
    context: &Bound<'py, PyAny>,
    methods: Methods,
    key: &Bound<'py, PyString>,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let py = context.py();
    let stock = if methods.get { dicts(context)? } else { None };
    let Some(dicts) = stock else {
        return match context.get_item(key) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.is_instance_of::<PyKeyError>(py) => Ok(None),
            Err(error) => Err(error),
        };
    };
    for index in (0..dicts.len()).rev() {
        // A layer's `__contains__` can change `dicts`; recheck the bound.
        let Ok(layer) = dicts.get_item(index) else {
            continue;
        };
        if let Some(layer) = plain_layer(&layer)? {
            if let Some(value) = layer.get_item(key)? {
                return Ok(Some(value));
            }
        } else if layer.contains(key)? {
            return layer.get_item(key).map(Some);
        }
    }
    Ok(None)
}

/// `context[key] = value`.
pub fn set(
    context: &Bound<'_, PyAny>,
    methods: Methods,
    key: &Bound<'_, PyString>,
    value: &Bound<'_, PyAny>,
) -> PyResult<()> {
    if methods.set
        && let Some(dicts) = dicts(context)?
        && let Ok(layer) = dicts.get_item(dicts.len().wrapping_sub(1))
    {
        return layer.set_item(key, value);
    }
    context.set_item(key, value)
}

/// `(autoescape, use_l10n, use_tz)` of a live context. A plain `Context` or
/// `RequestContext` keeps them in its instance `__dict__`.
pub fn flags(context: &Bound<'_, PyAny>) -> PyResult<(bool, Option<bool>, Option<bool>)> {
    let py = context.py();
    let names = (
        intern!(py, "autoescape"),
        intern!(py, "use_l10n"),
        intern!(py, "use_tz"),
    );
    let kind = context.get_type();
    if (kind.is(CONTEXT.import(py, "django.template.context", "Context")?)
        || kind.is(REQUEST_CONTEXT.import(py, "django.template.context", "RequestContext")?))
        && let Ok(attributes) = context.getattr(intern!(py, "__dict__"))
        && let Ok(attributes) = attributes.cast::<PyDict>()
        && let (Some(autoescape), Some(use_l10n), Some(use_tz)) = (
            attributes.get_item(names.0)?,
            attributes.get_item(names.1)?,
            attributes.get_item(names.2)?,
        )
    {
        return Ok((
            autoescape.extract()?,
            use_l10n.extract()?,
            use_tz.extract()?,
        ));
    }
    Ok((
        context.getattr(names.0)?.extract()?,
        context.getattr(names.1)?.extract()?,
        context.getattr(names.2)?.extract()?,
    ))
}
