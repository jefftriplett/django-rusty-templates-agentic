//! Synchronize a caller's Django Context only at a Python callable boundary.

use crate::gc::Gc;
use std::sync::Mutex;

use pyo3::prelude::*;
use pyo3::sync::MutexExt;
use pyo3::types::{PyDict, PyList};

use super::inheritance::{BlockCallback, BlockMarker};
use super::types::Context;

impl Context {
    /// # Panics
    ///
    /// Panics if a nested callback poisoned the shared context lock.
    pub fn call_variable<'py>(
        &mut self,
        py: Python<'py>,
        callable: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let Some(input) = self
            .callback_input
            .as_ref()
            .map(|reference| reference.bind(py).call0())
            .transpose()?
            .filter(|input| !input.is_none())
        else {
            return callable.call0();
        };
        let dicts = input.getattr("dicts")?.cast_into::<PyList>()?;
        if self.is_live_view_of(py, &dicts) {
            return self.call_with_live_view(py, callable, &input, &dicts);
        }
        let original_flags = (
            input.getattr("autoescape")?,
            input.getattr("use_l10n")?,
            input.getattr("use_tz")?,
        );
        let original_template = input.getattr("template")?;
        let original_depth = dicts.len();
        let shared = Gc::new(Mutex::new(std::mem::take(self)));
        let result = (|| -> PyResult<Bound<'py, PyAny>> {
            let overlay = {
                let context = shared
                    .lock_py_attached(py)
                    .expect("context lock is not poisoned");
                let overlay = context.callback_snapshot(py)?;
                if context.get_for_loop(0).is_some() {
                    overlay.set_item("forloop", context.loop_dictionary(py, 0)?)?;
                }
                if let Some(block) = context.get(py, "block")
                    && let Ok(marker) = block.extract::<PyRef<'_, BlockMarker>>()
                {
                    overlay.set_item(
                        "block",
                        Bound::new(
                            py,
                            BlockCallback {
                                marker: marker.clone(),
                                context: shared.clone(),
                            },
                        )?,
                    )?;
                }
                input.setattr("autoescape", context.autoescape)?;
                input.setattr("use_l10n", context.use_l10n)?;
                input.setattr("use_tz", context.use_tz)?;
                if let Some(root) = &context.root_template {
                    input.setattr("template", Py::new(py, root.as_ref().clone())?)?;
                }
                drop(context);
                overlay
            };
            let before = overlay.copy()?;
            dicts.append(&overlay)?;
            let result = callable.call0();
            let synchronization = (|| -> PyResult<()> {
                let after = input.call_method0("flatten")?.cast_into::<PyDict>()?;
                let mut context = shared
                    .lock_py_attached(py)
                    .expect("context lock is not poisoned");
                for (key, value) in after.iter() {
                    if before
                        .get_item(&key)?
                        .is_none_or(|before| !before.is(&value))
                    {
                        context.insert(key.extract()?, value)?;
                    }
                }
                for key in before.keys().iter() {
                    if !after.contains(&key)? {
                        context.clear_variable(&key.extract::<String>()?);
                    }
                }
                context.autoescape = input.getattr("autoescape")?.extract()?;
                context.use_l10n = input.getattr("use_l10n")?.extract()?;
                context.use_tz = input.getattr("use_tz")?.extract()?;
                drop(context);
                Ok(())
            })();
            let value = result?;
            synchronization?;
            Ok(value)
        })();
        let cleanup = (|| -> PyResult<()> {
            dicts.del_slice(original_depth, dicts.len())?;
            input.setattr("autoescape", original_flags.0)?;
            input.setattr("use_l10n", original_flags.1)?;
            input.setattr("use_tz", original_flags.2)?;
            input.setattr("template", original_template)?;
            Ok(())
        })();
        *self = shared.take_or_clone(py, |context| context.clone_ref(py));
        let value = result?;
        cleanup?;
        Ok(value)
    }

    /// `call_variable` when `input` already shows this context's values.
    /// Writes to the caller's dict are read live afterwards; values in
    /// contexts the callback pushed are copied in, as the snapshot path does,
    /// and the pushed contexts are removed.
    fn call_with_live_view<'py>(
        &mut self,
        py: Python<'py>,
        callable: &Bound<'py, PyAny>,
        input: &Bound<'py, PyAny>,
        dicts: &Bound<'py, PyList>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let original_flags = (
            input.getattr("autoescape")?,
            input.getattr("use_l10n")?,
            input.getattr("use_tz")?,
        );
        let original_template = input.getattr("template")?;
        let original_depth = dicts.len();
        // Django binds the template to the context and its render context
        // for the whole render; callbacks may read either.
        let render_context = input.getattr("render_context")?;
        let original_render_template = render_context.getattr("template")?;
        let setup = (|| -> PyResult<()> {
            input.setattr("autoescape", self.autoescape)?;
            input.setattr("use_l10n", self.use_l10n)?;
            input.setattr("use_tz", self.use_tz)?;
            if let Some(root) = &self.root_template {
                let template = Py::new(py, root.as_ref().clone())?;
                input.setattr("template", &template)?;
                render_context.setattr("template", template)?;
            }
            Ok(())
        })();
        let result = setup.and_then(|()| callable.call0());
        let synchronization = (|| -> PyResult<()> {
            for layer in dicts.iter().skip(original_depth) {
                for (key, value) in layer.cast_into::<PyDict>()?.iter() {
                    self.insert(key.extract()?, value)?;
                }
            }
            self.autoescape = input.getattr("autoescape")?.extract()?;
            self.use_l10n = input.getattr("use_l10n")?.extract()?;
            self.use_tz = input.getattr("use_tz")?.extract()?;
            Ok(())
        })();
        let cleanup = (|| -> PyResult<()> {
            dicts.del_slice(original_depth, dicts.len())?;
            input.setattr("autoescape", original_flags.0)?;
            input.setattr("use_l10n", original_flags.1)?;
            input.setattr("use_tz", original_flags.2)?;
            input.setattr("template", original_template)?;
            render_context.setattr("template", original_render_template)?;
            Ok(())
        })();
        let value = result?;
        synchronization?;
        cleanup?;
        Ok(value)
    }
}
