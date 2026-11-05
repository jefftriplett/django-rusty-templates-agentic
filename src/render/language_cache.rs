use std::sync::Mutex;

use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::{MutexExt, PyOnceLock};
use pyo3::types::PyString;

static GET_LANGUAGE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static CATALOG: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

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
        let catalog = CATALOG
            .import(py, "django.utils.translation.trans_real", "catalog")?
            .call0()?;
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
    ) -> PyResult<Option<&T>> {
        let language = GET_LANGUAGE
            .import(py, "django.utils.translation.trans_real", "catalog")?
            .call0()?;
        let Ok(language) = language.cast_into::<PyString>() else {
            return Ok(None);
        };
        let code = language.to_str()?;
        if self.entry.as_ref().is_none_or(|(cached, _)| cached != code) {
            self.entry = Some((code.to_owned(), init(&language)?));
        }
        Ok(self.entry.as_ref().map(|(_, value)| value))
    }
}
