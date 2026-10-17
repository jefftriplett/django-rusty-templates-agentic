//! Native inheritance state and block scopes. Python is used only at loader,
//! callback, and unsupported-template boundaries.

use crate::gc::{Gc, Trace};
use pyo3::{PyTraverseError, PyVisit};
use std::sync::Mutex;

use pyo3::prelude::*;
use pyo3::sync::MutexExt;
use pyo3::types::{PyDict, PyList};

use super::types::Context;
use crate::parse::TokenTree;
use crate::render::RenderResult;
use crate::template::django_rusty_templates::{PyEngine, Template, TemplateSyntaxError};

#[derive(Clone, Debug)]
pub struct BlockEntry {
    pub name: String,
    pub owner: Gc<Template>,
    pub nodes: Gc<Vec<TokenTree>>,
}

impl Trace for BlockEntry {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.owner.trace(visit)?;
        self.nodes.trace(visit)
    }
}

#[derive(Clone, Debug)]
#[pyclass(skip_from_py_object)]
pub struct BlockMarker {
    pub entry: BlockEntry,
    pub inherited: bool,
}

#[pymethods]
impl BlockMarker {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.entry.trace(&visit)
    }

    #[getter]
    fn name(&self) -> &str {
        &self.entry.name
    }
}

/// A Python callback can call block.super while the native context is owned by
/// `PyContext`. Ordinary template expressions resolve it directly in Rust.
#[pyclass]
pub struct BlockCallback {
    pub marker: BlockMarker,
    pub context: Gc<Mutex<Context>>,
}

#[pymethods]
impl BlockCallback {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.marker.entry.trace(&visit)?;
        self.context.trace(&visit)
    }

    #[getter]
    fn name(&self) -> &str {
        &self.marker.entry.name
    }

    #[pyo3(name = "super")]
    fn render_super(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let output = self
            .context
            .lock_py_attached(py)
            .expect("context lock is not poisoned")
            .render_super(py, &self.marker)
            .map_err(|error| self.marker.entry.owner.render_error(error))?;
        Ok(py
            .import("django.utils.safestring")?
            .getattr("mark_safe")?
            .call1((output,))?
            .unbind())
    }
}

impl Context {
    pub fn add_blocks(&mut self, owner: &Gc<Template>) {
        for (name, nodes) in owner.native_blocks() {
            self.inheritance_blocks
                .entry(name.clone())
                .or_default()
                .insert(
                    0,
                    BlockEntry {
                        name: name.clone(),
                        owner: owner.clone(),
                        nodes: nodes.clone(),
                    },
                );
        }
    }

    pub fn render_block(&mut self, py: Python<'_>, own: BlockEntry) -> RenderResult<'static> {
        let mut output = String::new();
        self.render_block_into(py, own, &mut output)?;
        Ok(std::borrow::Cow::Owned(output))
    }

    pub fn render_block_into(
        &mut self,
        py: Python<'_>,
        own: BlockEntry,
        output: &mut String,
    ) -> crate::render::RenderIntoResult {
        let inherited = self.inheritance_blocks.contains_key(&own.name);
        let popped = self
            .inheritance_blocks
            .get_mut(&own.name)
            .and_then(Vec::pop);
        let selected = popped.as_ref().unwrap_or(&own).clone();
        let mut scoped = self.clone_scope(py);
        let marker = BlockMarker {
            entry: selected.clone(),
            inherited,
        };
        scoped.insert("block".to_string(), Bound::new(py, marker)?.into_any())?;
        let result = selected.owner.render_body_with_owner_into(
            py,
            &selected.nodes,
            &mut scoped,
            output,
            Some(&selected.owner),
        );
        if let Some(popped) = popped {
            self.inheritance_blocks
                .entry(own.name)
                .or_default()
                .push(popped);
        }
        self.merge_render_state(py, scoped, &["block".to_string()])?;
        result
    }

    pub fn render_super(
        &mut self,
        py: Python<'_>,
        marker: &BlockMarker,
    ) -> Result<String, crate::error::PyRenderError> {
        if !marker.inherited {
            return Err(TemplateSyntaxError::new_err(
                "'BlockNode' object has no attribute 'context'. Did you use {{ block.super }} in a base template?",
            ).into());
        }
        if self
            .inheritance_blocks
            .get(&marker.entry.name)
            .is_none_or(Vec::is_empty)
        {
            return Ok(String::new());
        }
        Ok(self.render_block(py, marker.entry.clone())?.into_owned())
    }

    /// # Panics
    ///
    /// Panics if no template is currently being rendered.
    pub fn render_parent(
        &mut self,
        py: Python<'_>,
        parent: Bound<'_, PyAny>,
        token: &str,
        variable: bool,
    ) -> RenderResult<'static> {
        let current = self
            .current_template
            .as_ref()
            .expect("rendering a template")
            .clone();
        let parent = parent.getattr("template").unwrap_or(parent);
        if !parent.is_truthy()? {
            let mut message = format!(
                "Invalid template name in 'extends' tag: {}.",
                parent.repr()?
            );
            if variable {
                message.push_str(&format!(" Got this from the '{token}' variable."));
            }
            return Err(TemplateSyntaxError::new_err(message).into());
        }
        if let Some(context) = &self.formatting_context {
            let output = py
                .import("django_rusty_templates.inheritance")?
                .getattr("render_native_parent")?
                .call1((
                    parent,
                    current.compiled_blocks(py)?,
                    current.origin(),
                    context.bind(py),
                ))?
                .extract()?;
            return Ok(std::borrow::Cow::Owned(output));
        }
        let parent =
            if parent.extract::<PyRef<'_, Template>>().is_ok() || parent.hasattr("_render")? {
                parent
            } else {
                if self.inheritance_history.is_empty() {
                    self.inheritance_history.push(Gc::new(
                        current.origin().into_pyobject(py)?.into_any().unbind(),
                    ));
                }
                let engine = &self.root_template.as_ref().unwrap_or(&current).engine;
                let cached = parent
                    .extract::<String>()
                    .ok()
                    .and_then(|name| engine.cached_native_template(py, &name));
                if let Some(cached) = cached {
                    let origin = cached.origin();
                    // Compare full origins, not just template names: aliases
                    // and same-name overrides must still honor Django skips.
                    if !self
                        .inheritance_history
                        .iter()
                        .any(|previous| origin.matches(previous.bind(py)))
                    {
                        self.inheritance_history
                            .push(Gc::new(origin.into_pyobject(py)?.into_any().unbind()));
                        self.add_blocks(&current);
                        if !cached.extends_parent() {
                            self.add_blocks(&Gc::new(cached.clone()));
                        }
                        return cached
                            .render_chain(py, self)
                            .map(|output| std::borrow::Cow::Owned(output.into_owned()));
                    }
                }
                let skip = PyList::empty(py);
                for origin in &self.inheritance_history {
                    skip.append(origin.bind(py))?;
                }
                let loaded = py
                    .import("django_rusty_templates.interop")?
                    .getattr("find_template")?
                    .call1((
                        PyEngine {
                            engine: engine.clone(),
                        },
                        parent,
                        skip,
                    ))?;
                self.inheritance_history
                    .push(Gc::new(loaded.get_item(1)?.unbind()));
                loaded.get_item(0)?
            };
        self.add_blocks(&current);
        if let Ok(native) = parent.extract::<PyRef<'_, Template>>() {
            let native = Gc::new(native.clone());
            if native.is_native() {
                if !native.extends_parent() {
                    self.add_blocks(&native);
                }
                return native
                    .render_chain(py, self)
                    .map(|output| std::borrow::Cow::Owned(output.into_owned()));
            }
        }
        // An unsupported parent requires Django's renderer. Export the native
        // override stack only at that boundary, never on the native path.
        let blocks = PyDict::new(py);
        for (name, entries) in &self.inheritance_blocks {
            let stack = PyList::empty(py);
            for entry in entries {
                let body = entry.owner.block_template(entry.nodes.clone());
                stack.append(
                    py.import("django_rusty_templates.inheritance")?
                        .getattr("make_block")?
                        .call1((name, Py::new(py, body)?))?,
                )?;
            }
            blocks.set_item(name, stack)?;
        }
        let history = PyList::empty(py);
        for origin in &self.inheritance_history {
            history.append(origin.bind(py))?;
        }
        let root = self.root_template.as_ref().unwrap_or(&current);
        let output = py
            .import("django_rusty_templates.inheritance")?
            .getattr("render_fallback_parent")?
            .call1((
                parent,
                blocks,
                self.django_context(py)?,
                Py::new(py, root.as_ref().clone())?,
                history,
            ))?
            .extract()?;
        Ok(std::borrow::Cow::Owned(output))
    }
}
