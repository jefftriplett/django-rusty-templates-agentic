use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyString;

static GET_LANGUAGE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

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
            .import(py, "django.utils.translation", "get_language")?
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
