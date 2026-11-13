use std::sync::Mutex;

use pyo3::exceptions::PyAttributeError;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::{MutexExt, PyOnceLock};
use pyo3::types::{PyBool, PyDict, PyModule, PyString, PyType};
use pyo3::{PyTraverseError, PyVisit};

static GET_LANGUAGE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static CATALOG: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static TRANS_REAL: PyOnceLock<Py<PyModule>> = PyOnceLock::new();
static CVAR: PyOnceLock<Option<Py<PyType>>> = PyOnceLock::new();

/// What `getattr(trans_real._active, "value", None)` returns, read directly
/// from asgiref's context variable. `trans_real._active` is an asgiref
/// `Local`, whose attribute access takes a lock and enters a context manager
/// on every read; that is most of the cost of `get_language()`. Its storage
/// is a `_CVar` holding a `ContextVar` of a dict that is copied on every
/// write, so reading the dict is exact. Returns `None` when the storage
/// isn't that shape, such as a thread-critical `Local` or another asgiref
/// version, leaving the caller to ask Django.
/// The active translation, as `getattr(trans_real._active, "value", None)`.
enum Active<'py> {
    /// A translation is active.
    Translation(Bound<'py, PyAny>),
    /// None is active, so Django falls back to its default.
    Default,
    /// The storage isn't asgiref's usual shape; ask Django.
    Unknown,
}

fn active_translation(py: Python<'_>) -> PyResult<Active<'_>> {
    let trans_real = TRANS_REAL
        .get_or_try_init(py, || {
            py.import("django.utils.translation.trans_real")
                .map(Bound::unbind)
        })?
        .bind(py);
    let cvar = CVAR.get_or_init(py, || {
        py.import("asgiref.local")
            .and_then(|module| module.getattr("_CVar"))
            .and_then(|cvar| Ok(cvar.cast_into::<PyType>()?.unbind()))
            .ok()
    });
    let Some(cvar) = cvar else {
        return Ok(Active::Unknown);
    };
    let active = trans_real.getattr(intern!(py, "_active"))?;
    let Ok(storage) = active.getattr(intern!(py, "_storage")) else {
        return Ok(Active::Unknown);
    };
    if !storage.get_type().is(cvar.bind(py)) {
        return Ok(Active::Unknown);
    }
    let storage = storage
        .getattr(intern!(py, "_data"))?
        .call_method1(intern!(py, "get"), (py.None(),))?;
    if storage.is_none() {
        return Ok(Active::Default);
    }
    let Ok(storage) = storage.cast::<PyDict>() else {
        return Ok(Active::Unknown);
    };
    Ok(
        match storage
            .get_item(intern!(py, "value"))?
            .filter(|value| !value.is_none())
        {
            Some(translation) => Active::Translation(translation),
            None => Active::Default,
        },
    )
}

/// Django's `get_language()`, reading the active translation directly.
pub fn get_language(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    let django = || {
        GET_LANGUAGE
            .import(py, "django.utils.translation", "get_language")?
            .call0()
    };
    match active_translation(py)? {
        Active::Unknown => django(),
        Active::Translation(translation) => {
            match translation.call_method0(intern!(py, "to_language")) {
                Err(error) if error.is_instance_of::<PyAttributeError>(py) => language_code(py),
                language => language,
            }
        }
        Active::Default => language_code(py),
    }
}

/// `settings.LANGUAGE_CODE`, Django's language when none is active.
fn language_code(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    SETTINGS
        .import(py, "django.conf", "settings")?
        .getattr(intern!(py, "LANGUAGE_CODE"))
}

/// Django's `trans_real.catalog()`, reading the active translation directly.
fn catalog(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    let django = || {
        CATALOG
            .import(py, "django.utils.translation.trans_real", "catalog")?
            .call0()
    };
    match active_translation(py)? {
        Active::Translation(translation) => Ok(translation),
        Active::Default => {
            // `catalog()` builds the default translation once.
            let default = TRANS_REAL
                .get(py)
                .map(|module| module.bind(py).getattr(intern!(py, "_default")))
                .transpose()?;
            match default {
                Some(default) if !default.is_none() => Ok(default),
                _ => django(),
            }
        }
        Active::Unknown => django(),
    }
}

/// The catalog `gettext` would use now, or `None` with `USE_I18N` off, when
/// Django's null translation is in use.
pub fn active_catalog(py: Python<'_>) -> PyResult<Option<Bound<'_, PyAny>>> {
    let settings = SETTINGS.import(py, "django.conf", "settings")?;
    if !settings.getattr(intern!(py, "USE_I18N"))?.is_truthy()? {
        return Ok(None);
    }
    catalog(py).map(Some)
}

/// Catalogs a node keeps at once, enough for a site's interleaved languages.
const CATALOG_CAPACITY: usize = 8;

/// A value derived from Django's active translation catalog, kept on a
/// template node across renders.
///
/// Entries are keyed by the identity of the catalog object `gettext` would
/// use, not by the language code. Activating a language selects that
/// language's catalog object, and Django replaces catalog objects whenever it
/// reloads translations: the autoreloader does so when a `.mo` file changes,
/// as do changes to settings such as `LOCALE_PATHS`. A reloaded catalog
/// therefore misses here instead of serving stale text. Each entry holds its
/// catalog, so a freed catalog's address cannot be reused for a match.
#[derive(Default)]
pub struct CatalogCache {
    entries: Mutex<Vec<(Py<PyAny>, Py<PyAny>)>>,
}

impl CatalogCache {
    /// Returns the value for the active catalog, computing it with `init`
    /// on a miss. With `USE_I18N` off, Django's null translation is in use and
    /// nothing is cached.
    pub fn get_or_try_init<'py, E: From<PyErr>>(
        &self,
        py: Python<'py>,
        init: impl FnOnce() -> Result<Bound<'py, PyAny>, E>,
    ) -> Result<Bound<'py, PyAny>, E> {
        let settings = SETTINGS.import(py, "django.conf", "settings")?;
        if !settings.getattr(intern!(py, "USE_I18N"))?.is_truthy()? {
            return init();
        }
        let catalog = catalog(py)?;
        let find = |entries: &[(Py<PyAny>, Py<PyAny>)]| {
            entries
                .iter()
                .find(|(cached, _)| cached.is(&catalog))
                .map(|(_, value)| value.bind(py).clone())
        };
        let cached = find(&self.lock(py));
        if let Some(value) = cached {
            return Ok(value);
        }
        // Compute without the lock: `init` runs Python code.
        let value = init()?;
        let mut entries = self.lock(py);
        if find(&entries).is_none() {
            if entries.len() == CATALOG_CAPACITY {
                entries.remove(0);
            }
            entries.push((catalog.unbind(), value.clone().unbind()));
        }
        drop(entries);
        Ok(value)
    }

    fn lock(&self, py: Python<'_>) -> std::sync::MutexGuard<'_, Vec<(Py<PyAny>, Py<PyAny>)>> {
        self.entries
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned")
    }
}

impl crate::gc::Trace for CatalogCache {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        // A collection can start while a render holds the lock; the entries
        // are then reachable from that render.
        if let Ok(entries) = self.entries.try_lock() {
            for (catalog, value) in entries.iter() {
                visit.call(catalog)?;
                visit.call(value)?;
            }
        }
        Ok(())
    }
}

// Cached text is derived state: copies start empty and never affect equality.
impl Clone for CatalogCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for CatalogCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for CatalogCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CatalogCache")
    }
}

/// A value derived from the active language, kept for the rest of a render.
///
/// Django recomputes values such as translated strings and number formats on
/// every filter call. A render usually keeps one language, so the value is
/// recomputed only when `get_language()` reports a different one, which also
/// covers `{% language %}` and Python code that activates a language.
#[derive(Clone, Debug)]
pub struct LanguageCache<T> {
    entry: Option<(String, T)>,
}

impl<T> Default for LanguageCache<T> {
    fn default() -> Self {
        Self { entry: None }
    }
}

impl<T> LanguageCache<T> {
    /// Returns the value for the active language, computing it with `init`
    /// when the language has changed. Returns `None` when translations are
    /// deactivated and `get_language()` returns `None`.
    pub fn get_or_try_init<'py>(
        &mut self,
        py: Python<'py>,
        init: impl FnOnce(&Bound<'py, PyString>) -> PyResult<T>,
    ) -> PyResult<Option<&mut T>> {
        let language = get_language(py)?;
        let Ok(language) = language.cast_into::<PyString>() else {
            return Ok(None);
        };
        let code = language.to_str()?;
        if self.entry.as_ref().is_none_or(|(cached, _)| cached != code) {
            self.entry = Some((code.to_owned(), init(&language)?));
        }
        Ok(self.entry.as_mut().map(|(_, value)| value))
    }
}

/// The fields of `get_language_info(code)` the i18n filters read.
#[derive(Clone, Debug)]
pub struct LanguageInfo {
    pub name: String,
    pub name_local: String,
    pub bidi: bool,
}

static GET_LANGUAGE_INFO: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

impl LanguageInfo {
    /// Calls Django's `get_language_info`. `None` when it raises or returns
    /// something other than plain strings and a bool, so the caller can let
    /// Django's filter raise or handle it.
    pub fn load(py: Python<'_>, code: &str) -> PyResult<Option<Self>> {
        let get_language_info =
            GET_LANGUAGE_INFO.import(py, "django.utils.translation", "get_language_info")?;
        let Ok(info) = get_language_info.call1((code,)) else {
            return Ok(None);
        };
        let field = |name: &str| -> Option<Bound<'_, PyAny>> { info.get_item(name).ok() };
        let text = |name: &str| -> Option<String> {
            let value = field(name)?;
            value
                .is_exact_instance_of::<PyString>()
                .then(|| value.extract().ok())
                .flatten()
        };
        let bidi = field("bidi").and_then(|value| {
            value
                .is_exact_instance_of::<PyBool>()
                .then(|| value.is_truthy().ok())
                .flatten()
        });
        Ok(match (text("name"), text("name_local"), bidi) {
            (Some(name), Some(name_local), Some(bidi)) => Some(Self {
                name,
                name_local,
                bidi,
            }),
            _ => None,
        })
    }
}
