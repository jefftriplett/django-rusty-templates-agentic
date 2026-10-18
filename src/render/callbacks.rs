//! Synchronize a caller's Django Context only at a Python callable boundary.

use crate::gc::Gc;
use std::sync::Mutex;

use pyo3::prelude::*;
use pyo3::sync::MutexExt;
use pyo3::types::{PyDict, PyList};

use super::inheritance::{BlockCallback, BlockMarker};
use super::types::Context;

impl Context {
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
        let original_flags = (
            input.getattr("autoescape")?,
            input.getattr("use_l10n")?,
            input.getattr("use_tz")?,
        );
        let original_template = input.getattr("template")?;
        let dicts = input.getattr("dicts")?.cast_into::<PyList>()?;
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
                if let Some(block) = context.get("block")
                    && let Ok(marker) = block.bind(py).extract::<PyRef<'_, BlockMarker>>()
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
        *self = shared
            .lock_py_attached(py)
            .expect("context lock is not poisoned")
            .clone_ref(py);
        let value = result?;
        cleanup?;
        Ok(value)
    }
}
