use crate::gc::{Gc, GcOnceLock, Trace};
use pyo3::{PyTraverseError, PyVisit};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;
use std::iter::zip;
use std::sync::Mutex;

use html_escape::{encode_quoted_attribute, encode_quoted_attribute_to_string};
use miette::SourceSpan;
use num_bigint::{BigInt, ToBigInt};
use num_traits::{ToPrimitive, Zero};
use pyo3::exceptions::{PyAttributeError, PyKeyError, PyTypeError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::{MutexExt, PyOnceLock};
use pyo3::types::{PyBool, PyDict, PyInt, PyString, PyType};

use super::inheritance::{BlockEntry, BlockMarker};
use crate::error::{AnnotatePyErr, PyRenderError, RenderError};
use crate::parse::CycleId;
use crate::render::RenderResult;
use crate::render::dateformat::DateCache;
use crate::template::django_rusty_templates::{Engine, Template, get_template, select_template};
use crate::utils::PyResultMethods;
use dtl_lexer::types::{At, TemplateString};

static DJANGO_CONTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static RENDER_VALUE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static CONDITIONAL_ESCAPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

static MARK_SAFE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

// Django's value formatter reads only these flags. Rendering a number must
// not materialize the template variables, loop dictionaries or block callbacks.
#[pyclass(frozen, get_all)]
struct ValueFormattingContext {
    autoescape: bool,
    use_l10n: Option<bool>,
    use_tz: Option<bool>,
}

// Two autoescape flags and three states for each optional flag. Each adapter
// is immutable, so retained Python references keep their original flags.
static VALUE_FORMATTING_CONTEXTS: [PyOnceLock<Py<ValueFormattingContext>>; 18] =
    [const { PyOnceLock::new() }; 18];

fn formatting_flag_index(flag: Option<bool>) -> usize {
    match flag {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

#[derive(Clone)]
pub struct ForLoop {
    count: usize,
    len: usize,
    dictionary: Gc<GcOnceLock<Py<PyDict>>>,
}

impl std::fmt::Debug for ForLoop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForLoop")
            .field("count", &self.count)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl ForLoop {
    pub fn length(&self) -> usize {
        self.len
    }

    pub fn counter0(&self) -> usize {
        self.count
    }

    pub fn counter(&self) -> usize {
        self.count + 1
    }

    pub fn rev_counter(&self) -> usize {
        self.len - self.count
    }

    pub fn rev_counter0(&self) -> usize {
        self.len - self.count - 1
    }

    pub fn first(&self) -> bool {
        self.count == 0
    }

    pub fn last(&self) -> bool {
        self.count + 1 == self.len
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IncludeTemplateKey {
    String(String),
    Vec(Vec<String>),
}

type BindingMap = HashMap<String, Vec<Py<PyAny>>>;

#[derive(Debug)]
struct SharedBindings(BindingMap);

impl Trace for SharedBindings {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.0.trace(visit)
    }
}

impl Trace for ForLoop {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.dictionary.trace(visit)
    }
}

impl Clone for SharedBindings {
    fn clone(&self) -> Self {
        Python::attach(|py| {
            Self(
                self.0
                    .iter()
                    .map(|(key, values)| {
                        (
                            key.clone(),
                            values.iter().map(|value| value.clone_ref(py)).collect(),
                        )
                    })
                    .collect(),
            )
        })
    }
}

#[derive(Clone, Debug)]
struct ScopeBindings {
    base: Gc<SharedBindings>,
    overlay: bool,
}

#[derive(Debug, Default)]
pub struct Context {
    context: BindingMap,
    scope_bindings: Option<ScopeBindings>,
    loops: Vec<ForLoop>,
    pub request: Option<Py<PyAny>>,
    pub autoescape: bool,
    pub use_l10n: Option<bool>,
    pub use_tz: Option<bool>,
    pub(crate) date_cache: DateCache,
    pub format_values: bool,
    pub instrument_render: bool,
    pub formatting_context: Option<Py<PyAny>>,
    // Weak reference to an input Context, used only while calling Python code.
    pub callback_input: Option<Gc<Py<PyAny>>>,
    pub inheritance_blocks: HashMap<String, Vec<BlockEntry>>,
    pub python_blocks: Option<Py<PyAny>>,
    pub current_template: Option<Gc<Template>>,
    pub root_template: Option<Gc<Template>>,
    pub inheritance_history: Vec<Gc<Py<PyAny>>>,
    // Share unchanged values across Python context snapshots. Each snapshot
    // owns a small overlay so later loop iterations cannot alter earlier ones.
    django_context_base: RefCell<Option<Py<PyDict>>>,
    django_context_changes: HashSet<String>,
    pub string_if_invalid: String,
    names: Vec<HashSet<String>>,
    include_cache: HashMap<(CycleId, IncludeTemplateKey), Gc<Template>>,
    pub(crate) cycle_indices: HashMap<CycleId, usize>,
    changed_states: Vec<HashMap<CycleId, Py<PyAny>>>,
    assigned: HashSet<String>,
    cycle_variables: HashSet<String>,
}

impl Trace for Context {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.context.trace(visit)?;
        if let Some(shared) = &self.scope_bindings {
            shared.base.trace(visit)?;
        }
        self.loops.trace(visit)?;
        self.request.trace(visit)?;
        self.formatting_context.trace(visit)?;
        self.date_cache.trace(visit)?;
        self.callback_input.trace(visit)?;
        self.inheritance_blocks.trace(visit)?;
        self.python_blocks.trace(visit)?;
        self.current_template.trace(visit)?;
        self.root_template.trace(visit)?;
        self.inheritance_history.trace(visit)?;
        if let Ok(base) = self.django_context_base.try_borrow() {
            base.trace(visit)?;
        }
        self.include_cache.trace(visit)?;
        self.changed_states.trace(visit)
    }
}

impl Trace for Mutex<Context> {
    fn count_native_owners(&self) -> bool {
        true
    }

    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        // A callback can trigger GC while a render is using this context.
        if let Ok(context) = self.try_lock() {
            context.trace(visit)?;
        }
        Ok(())
    }

    fn clear(&self) {
        let removed = self
            .try_lock()
            .ok()
            .map(|mut context| std::mem::take(&mut *context));
        // Finalizers may call back into the context; release its lock first.
        drop(removed);
    }
}

impl Context {
    pub fn new(
        context: HashMap<String, Py<PyAny>>,
        request: Option<Py<PyAny>>,
        autoescape: bool,
    ) -> Self {
        let context = context.into_iter().map(|(k, v)| (k, vec![v])).collect();
        Self {
            request,
            context,
            scope_bindings: None,
            autoescape,
            use_l10n: None,
            use_tz: None,
            date_cache: DateCache::default(),
            format_values: false,
            instrument_render: true,
            formatting_context: None,
            callback_input: None,
            inheritance_blocks: HashMap::new(),
            python_blocks: None,
            current_template: None,
            root_template: None,
            inheritance_history: Vec::new(),
            django_context_base: RefCell::new(None),
            django_context_changes: HashSet::new(),
            string_if_invalid: String::new(),
            loops: Vec::new(),
            names: Vec::new(),
            include_cache: HashMap::new(),
            cycle_indices: HashMap::new(),
            changed_states: vec![HashMap::new()],
            assigned: HashSet::new(),
            cycle_variables: HashSet::new(),
        }
    }

    #[must_use]
    pub fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            request: self.request.as_ref().map(|v| v.clone_ref(py)),
            context: self
                .context
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().map(|v| v.clone_ref(py)).collect()))
                .collect(),
            scope_bindings: self.scope_bindings.clone(),
            autoescape: self.autoescape,
            use_l10n: self.use_l10n,
            use_tz: self.use_tz,
            date_cache: DateCache::default(),
            format_values: self.format_values,
            instrument_render: self.instrument_render,
            formatting_context: self.formatting_context.as_ref().map(|v| v.clone_ref(py)),
            callback_input: self.callback_input.clone(),
            inheritance_blocks: self.inheritance_blocks.clone(),
            python_blocks: self.python_blocks.as_ref().map(|v| v.clone_ref(py)),
            current_template: self.current_template.clone(),
            root_template: self.root_template.clone(),
            inheritance_history: self.inheritance_history.clone(),
            django_context_base: RefCell::new(None),
            django_context_changes: HashSet::new(),
            string_if_invalid: self.string_if_invalid.clone(),
            loops: self.loops.clone(),
            names: self.names.clone(),
            include_cache: self.include_cache.clone(),
            cycle_indices: self.cycle_indices.clone(),
            assigned: self.assigned.clone(),
            cycle_variables: self.cycle_variables.clone(),
            changed_states: self
                .changed_states
                .iter()
                .map(|frame| {
                    frame
                        .iter()
                        .map(|(key, value)| (*key, value.clone_ref(py)))
                        .collect()
                })
                .collect(),
        }
    }

    /// Freeze bindings once per render; child writes copy only the affected stack.
    /// Parent writes use copy-on-write so retained callback snapshots stay valid.
    pub(crate) fn clone_scope(&mut self, py: Python<'_>) -> Self {
        if self.scope_bindings.is_none() {
            self.scope_bindings = Some(ScopeBindings {
                base: Gc::new(SharedBindings(std::mem::take(&mut self.context))),
                overlay: false,
            });
        }
        let mut scoped = self.clone_ref(py);
        scoped
            .scope_bindings
            .as_mut()
            .expect("scope initialized")
            .overlay = true;
        scoped
    }

    fn visible_bindings(&self) -> impl Iterator<Item = (&String, &Vec<Py<PyAny>>)> {
        self.scope_bindings
            .as_ref()
            .into_iter()
            .flat_map(|shared| shared.base.0.iter())
            .filter(|(key, _)| !self.context.contains_key(*key))
            .chain(self.context.iter())
    }

    fn bindings_mut(&mut self) -> &mut BindingMap {
        if let Some(shared) = self.scope_bindings.take_if(|shared| !shared.overlay) {
            // Materialize parent writes while retained child snapshots keep
            // their immutable GC-visible base.
            let mut bindings = (*shared.base).clone().0;
            bindings.extend(std::mem::take(&mut self.context));
            self.context = bindings;
        }
        &mut self.context
    }

    fn has_binding_key(&self, key: &str) -> bool {
        self.context.contains_key(key)
            || self
                .scope_bindings
                .as_ref()
                .is_some_and(|shared| shared.base.0.contains_key(key))
    }

    pub fn invalid_value<'py>(
        &self,
        py: Python<'py>,
        variable: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let value = PyString::new(py, &self.string_if_invalid);
        if let Some(variable) = variable
            && self.string_if_invalid.contains("%s")
        {
            value.call_method1("__mod__", (variable,))
        } else {
            Ok(value.into_any())
        }
    }

    pub fn as_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (key, values) in self.visible_bindings() {
            if let Some(value) = values.last() {
                dict.set_item(key, value)?;
            }
        }
        Ok(dict)
    }

    /// Callback writes already affect a caller-owned Django Context. Only
    /// native assignment keys need refreshing before the final writeback.
    pub(crate) fn refresh_assignments(
        &mut self,
        py: Python<'_>,
        live: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let names: Vec<_> = self.assigned.iter().cloned().collect();
        for name in names {
            match live.get_item(&name) {
                Ok(value) => self._insert(name, value, true),
                Err(error) if error.is_instance_of::<PyKeyError>(py) => {
                    self.assigned.remove(&name);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn write_assignments(&self, py: Python<'_>, target: &Bound<'_, PyAny>) -> PyResult<()> {
        for key in &self.assigned {
            if let Some(value) = self.get(key) {
                target.set_item(key, value.bind(py))?;
            }
        }
        Ok(())
    }

    /// Materialize a retained callback snapshot using the immutable base and
    /// only the bindings changed by native scopes. Python may mutate or retain
    /// the returned dict, so every callback still gets its own copy.
    pub(crate) fn callback_snapshot<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let mut cached = self.django_context_base.borrow_mut();
        if cached.is_none() {
            *cached = Some(self.as_dict(py)?.unbind());
        }
        let snapshot = cached.as_ref().expect("base initialized").bind(py).copy()?;
        for name in &self.django_context_changes {
            if let Some(value) = self.get(name) {
                snapshot.set_item(name, value)?;
            } else if snapshot.contains(name)? {
                snapshot.del_item(name)?;
            }
        }
        Ok(snapshot)
    }

    pub fn django_context<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if let Some(context) = &self.formatting_context {
            return Ok(context.bind(py).clone());
        }
        let kwargs = PyDict::new(py);
        kwargs.set_item("autoescape", self.autoescape)?;
        kwargs.set_item("use_l10n", self.use_l10n)?;
        kwargs.set_item("use_tz", self.use_tz)?;
        let mut cached_base = self.django_context_base.borrow_mut();
        let base = if let Some(base) = cached_base.as_ref() {
            base
        } else {
            let base = PyDict::new(py);
            for (key, values) in self.visible_bindings() {
                if let Some(value) = values.last() {
                    base.set_item(key, value)?;
                }
            }
            cached_base.insert(base.unbind())
        }
        .bind(py);
        let overlay = PyDict::new(py);
        if !self.loops.is_empty() {
            overlay.set_item("forloop", self.loop_dictionary(py, 0)?)?;
        }
        if let Some(value) = self.get("block")
            && let Ok(marker) = value.bind(py).extract::<PyRef<'_, BlockMarker>>()
        {
            overlay.set_item(
                "block",
                Bound::new(
                    py,
                    super::inheritance::BlockCallback {
                        marker: marker.clone(),
                        context: Gc::new(Mutex::new(self.clone_ref(py))),
                    },
                )?,
            )?;
        }
        let mut trimmed_base = None;
        for key in &self.django_context_changes {
            if let Some(value) = self.get(key) {
                overlay.set_item(key, value)?;
            } else if base.contains(key)? {
                // A callback can remove a value that was originally in the
                // base. Copy only in that case, preserving older snapshots.
                let trimmed = match &trimmed_base {
                    Some(trimmed) => trimmed,
                    None => trimmed_base.insert(base.copy()?),
                };
                trimmed.del_item(key)?;
            }
        }
        let context = DJANGO_CONTEXT
            .import(py, "django.template.context", "Context")?
            .call((trimmed_base.as_ref().unwrap_or(base),), Some(&kwargs))?;
        if let Some(trimmed) = trimmed_base {
            // Keep the trimmed base for later snapshots too. Scoped values
            // present at the first conversion must stay absent after a pop.
            *cached_base = Some(trimmed.unbind());
        }
        context
            .getattr("dicts")?
            .cast::<pyo3::types::PyList>()?
            .append(overlay)?;
        if let Some(root) = &self.root_template {
            context.setattr("template", Py::new(py, root.as_ref().clone())?)?;
            context.setattr("template_name", root.template_name.as_deref())?;
        }
        if let Some(request) = &self.request {
            context.setattr("request", request)?;
        }
        Ok(context)
    }

    pub(crate) fn value_formatting_context<'py>(
        &self,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        if let Some(context) = &self.formatting_context {
            return Ok(context.bind(py).clone());
        }
        let index = usize::from(self.autoescape) * 9
            + formatting_flag_index(self.use_l10n) * 3
            + formatting_flag_index(self.use_tz);
        VALUE_FORMATTING_CONTEXTS[index]
            .get_or_try_init(py, || {
                Py::new(
                    py,
                    ValueFormattingContext {
                        autoescape: self.autoescape,
                        use_l10n: self.use_l10n,
                        use_tz: self.use_tz,
                    },
                )
            })
            .map(|context| context.bind(py).clone().into_any())
    }

    pub fn get(&self, key: &str) -> Option<&Py<PyAny>> {
        if let Some(values) = self.context.get(key) {
            return values.last();
        }
        self.scope_bindings.as_ref()?.base.0.get(key)?.last()
    }

    pub fn resolve_root<'py>(
        &self,
        py: Python<'py>,
        key: &str,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        if let Some(context) = &self.formatting_context {
            return match context.bind(py).get_item(key) {
                Ok(value) => Ok(Some(value)),
                Err(error) if error.is_instance_of::<PyKeyError>(py) => Ok(None),
                Err(error) => Err(error),
            };
        }
        if key == "forloop" && !self.loops.is_empty() {
            return self.loop_dictionary(py, 0).map(Some);
        }
        Ok(self.get(key).map(|value| value.bind(py).clone()))
    }

    /// Use the owning engine's version snapshot while rendering a template.
    /// Standalone contexts retain Django lookup for low-level callers.
    pub(crate) fn django_series(&self, py: Python<'_>) -> PyResult<(u8, u8)> {
        if let Some(template) = &self.current_template {
            return Ok(template.engine.django_series);
        }
        let version = py.import("django")?.getattr("VERSION")?;
        Ok((
            version.get_item(0)?.extract()?,
            version.get_item(1)?.extract()?,
        ))
    }

    pub fn refresh_formatting_flags(&mut self, py: Python<'_>) -> PyResult<()> {
        if let Some(context) = &self.formatting_context {
            let context = context.bind(py);
            self.autoescape = context.getattr("autoescape")?.extract()?;
            self.use_l10n = context.getattr("use_l10n")?.extract()?;
            self.use_tz = context.getattr("use_tz")?.extract()?;
        }
        Ok(())
    }

    pub fn display(&self, py: Python<'_>) -> String {
        let context: BTreeMap<_, _> = self
            .visible_bindings()
            .filter_map(|(k, v)| Some((k, v.last()?.bind(py))))
            .collect();
        format!("{context:?}")
    }

    fn _insert(&mut self, key: String, value: Bound<'_, PyAny>, replace: bool) {
        // Pure native renders never need a Python snapshot. Start tracking
        // mutations only after the first conversion has captured a base.
        if self.django_context_base.get_mut().is_some() {
            self.django_context_changes.insert(key.clone());
        }
        let inherited = self
            .scope_bindings
            .as_ref()
            .filter(|shared| shared.overlay)
            .map(|shared| shared.base.clone());
        let py = value.py();
        let value = value.unbind();
        match self.bindings_mut().entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let values = entry.get_mut();
                if replace {
                    values.pop();
                }
                values.push(value);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut values: Vec<_> = inherited
                    .as_ref()
                    .and_then(|base| base.0.get(entry.key()))
                    .map(|values| values.iter().map(|value| value.clone_ref(py)).collect())
                    .unwrap_or_default();
                if replace {
                    values.pop();
                }
                values.push(value);
                entry.insert(values);
            }
        }
    }

    pub fn append(&mut self, key: String, value: Bound<'_, PyAny>) {
        self._insert(key, value, false);
    }

    pub fn insert(&mut self, key: String, value: Bound<'_, PyAny>) -> PyResult<()> {
        if let Some(context) = &self.formatting_context {
            context.bind(value.py()).set_item(&key, &value)?;
        }
        self.assigned.insert(key.clone());
        let replace = self
            .names
            .last_mut()
            .is_none_or(|names| !names.insert(key.clone()));
        self._insert(key, value, replace);
        Ok(())
    }

    pub(crate) fn clear_variable(&mut self, key: &str) {
        if self
            .scope_bindings
            .as_ref()
            .is_some_and(|shared| shared.overlay)
        {
            self.context.entry(key.to_string()).or_default().clear();
        } else if let Some(values) = self.bindings_mut().get_mut(key) {
            values.clear();
        }
        if self.django_context_base.get_mut().is_some() {
            self.django_context_changes.insert(key.to_string());
        }
    }

    pub fn push_variable(&mut self, name: String, value: Bound<'_, PyAny>, index: usize) {
        let replace = index != 0;
        if !replace {
            let mut names_set = HashSet::new();
            names_set.insert(name.clone());
            self.names.push(names_set);
        }
        self._insert(name, value, replace);
    }

    pub fn push_variables(
        &mut self,
        names: &Vec<String>,
        names_at: At,
        values: Bound<'_, PyAny>,
        values_at: At,
        index: usize,
        template: TemplateString<'_>,
    ) -> Result<(), PyRenderError> {
        let replace = index != 0;
        if !replace {
            let names_set = names.iter().cloned().collect();
            self.names.push(names_set);
        }
        if names.len() == 1 {
            self._insert(names[0].clone(), values, replace);
        } else {
            let py = values.py();
            let values: Vec<_> = match values.try_iter() {
                Ok(values) => match values.collect() {
                    Ok(values) => values,
                    Err(error) => {
                        let error = error.annotate(py, values_at, "while unpacking this", template);
                        return Err(error.into());
                    }
                },
                Err(error) if error.is_instance_of::<PyTypeError>(py) => {
                    return Err(RenderError::TupleUnpackError {
                        expected_count: names.len(),
                        actual_count: 1,
                        expected_at: names_at.into(),
                        actual_at: values_at.into(),
                    }
                    .into());
                }
                Err(error) => {
                    let error = error.annotate(py, values_at, "while iterating this", template);
                    return Err(error.into());
                }
            };
            if names.len() == values.len() {
                for (name, value) in zip(names, values) {
                    self._insert(name.clone(), value, replace);
                }
            } else {
                return Err(RenderError::TupleUnpackError {
                    expected_count: names.len(),
                    actual_count: values.len(),
                    expected_at: names_at.into(),
                    actual_at: values_at.into(),
                }
                .into());
            }
        }
        Ok(())
    }

    /// # Panics
    ///
    /// Panics if `name` was never pushed.
    pub fn pop_variable(&mut self, name: &str) {
        if self.django_context_base.get_mut().is_some() {
            self.django_context_changes.insert(name.to_string());
        }
        let values = self
            .bindings_mut()
            .get_mut(name)
            .expect("Variable should have been pushed before");
        values.pop();
    }

    pub(crate) fn push_scope(&mut self, names: &[&str]) {
        self.names
            .push(names.iter().map(|name| (*name).to_string()).collect());
    }

    pub fn pop_variables(&mut self) {
        if let Some(names) = self.names.pop() {
            for name in names {
                self.pop_variable(&name);
            }
        }
    }

    pub fn push_for_loop(&mut self, len: usize) {
        self.loops.push(ForLoop {
            count: 0,
            len,
            dictionary: Gc::new(GcOnceLock::new()),
        });
        self.changed_states.push(HashMap::new());
    }

    /// # Panics
    ///
    /// Panics if no for loop is active.
    pub fn increment_for_loop(&mut self) {
        let for_loop = self
            .loops
            .last_mut()
            .expect("Called within an active for loop");
        for_loop.count += 1;
    }

    /// # Panics
    ///
    /// Panics if no for loop is active.
    pub fn pop_for_loop(&mut self) {
        self.changed_states.pop();
        self.loops
            .pop()
            .expect("Called when exiting an active for loop");
    }

    pub fn get_for_loop(&self, depth: usize) -> Option<&ForLoop> {
        let index = self.loops.len().checked_sub(depth + 1)?;
        self.loops.get(index)
    }

    pub fn loop_dictionary_cached(&self, _py: Python<'_>, depth: usize) -> bool {
        self.get_for_loop(depth)
            .is_some_and(|loop_| loop_.dictionary.get().is_some())
    }

    fn fill_loop_dictionary(loop_: &ForLoop, dict: &Bound<'_, PyDict>) -> PyResult<()> {
        dict.set_item("counter", loop_.counter())?;
        dict.set_item("counter0", loop_.counter0())?;
        dict.set_item("revcounter", loop_.rev_counter())?;
        dict.set_item("revcounter0", loop_.rev_counter0())?;
        dict.set_item("first", loop_.first())?;
        dict.set_item("last", loop_.last())?;
        Ok(())
    }

    pub fn loop_dictionary<'py>(
        &self,
        py: Python<'py>,
        depth: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let Some(loop_) = self.get_for_loop(depth) else {
            return Ok(self.get("forloop").map_or_else(
                || PyDict::new(py).into_any(),
                |value| value.bind(py).clone(),
            ));
        };
        loop_
            .dictionary
            .get_or_try_init(py, || {
                let dict = PyDict::new(py);
                dict.set_item("parentloop", self.loop_dictionary(py, depth + 1)?)?;
                Self::fill_loop_dictionary(loop_, &dict)?;
                if self.django_series(py)? >= (6, 0) {
                    dict.set_item("length", loop_.length())?;
                }
                Ok(dict.unbind())
            })
            .map(|dict| dict.bind(py).clone().into_any())
    }

    /// Expose native loop scopes to Python callbacks on the live Django context.
    ///
    /// # Panics
    ///
    /// Panics if called for a live Django context while no for loop is active.
    pub fn sync_loop_context(&self, py: Python<'_>, first: bool) -> PyResult<()> {
        let Some(context) = &self.formatting_context else {
            if let Some(loop_) = self.loops.last()
                && let Some(dict) = loop_.dictionary.get()
            {
                Self::fill_loop_dictionary(loop_, dict.bind(py))?;
            }
            return Ok(());
        };
        let context = context.bind(py);
        if let Some(names) = self.names.last() {
            for key in names {
                if let Some(value) = self.get(key) {
                    context.set_item(key, value)?;
                }
            }
        }
        let dict = if first {
            let parent = context
                .get_item("forloop")
                .unwrap_or_else(|_| PyDict::new(py).into_any());
            let dict = PyDict::new(py);
            dict.set_item("parentloop", parent)?;
            context.set_item("forloop", &dict)?;
            dict.into_any()
        } else {
            context.get_item("forloop")?
        };
        let for_loop = self.loops.last().expect("inside native loop");
        dict.set_item("counter", for_loop.counter())?;
        dict.set_item("counter0", for_loop.counter0())?;
        dict.set_item("revcounter", for_loop.rev_counter())?;
        dict.set_item("revcounter0", for_loop.rev_counter0())?;
        dict.set_item("first", for_loop.first())?;
        dict.set_item("last", for_loop.last())?;
        if first && self.django_series(py)? >= (6, 0) {
            dict.set_item("length", for_loop.length())?;
        }
        Ok(())
    }

    #[expect(
        clippy::missing_panics_doc,
        reason = "setting str keys and formatting a dict of builtins cannot fail"
    )]
    pub fn render_for_loop(&self, py: Python<'_>, depth: usize) -> String {
        let mut forloop_dict = PyDict::new(py);
        for forloop in self.loops.iter().rev().take(self.loops.len() - depth) {
            let dict = PyDict::new(py);
            dict.set_item("parentloop", forloop_dict)
                .expect("Can always set a str: dict key/value");
            if self.django_series(py).is_ok_and(|series| series >= (6, 0)) {
                dict.set_item("length", forloop.length())
                    .expect("Can always set a str: int key/value");
            }
            dict.set_item("counter0", forloop.counter0())
                .expect("Can always set a str: int key/value");
            dict.set_item("counter", forloop.counter())
                .expect("Can always set a str: int key/value");
            dict.set_item("revcounter", forloop.rev_counter())
                .expect("Can always set a str: int key/value");
            dict.set_item("revcounter0", forloop.rev_counter0())
                .expect("Can always set a str: int key/value");
            dict.set_item("first", forloop.first())
                .expect("Can always set a str: bool key/value");
            dict.set_item("last", forloop.last())
                .expect("Can always set a str: bool key/value");
            forloop_dict = dict;
        }

        let forloop_str = forloop_dict
            .str()
            .expect("All elements of the dictionary can be converted to a string");
        forloop_str.to_string()
    }

    pub fn get_or_insert_include(
        &mut self,
        py: Python,
        engine: &Gc<Engine>,
        key: &IncludeTemplateKey,
        id: CycleId,
    ) -> Result<Gc<Template>, PyErr> {
        match self.include_cache.entry((id, key.clone())) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                let include = match key {
                    IncludeTemplateKey::String(content) => {
                        get_template(engine.clone(), py, Cow::Borrowed(content))?
                    }
                    IncludeTemplateKey::Vec(templates) => {
                        select_template(engine.clone(), py, templates.clone())?
                    }
                };
                Ok(entry.insert(Gc::new(include)).clone())
            }
        }
    }

    /// # Panics
    ///
    /// Panics if the `ifchanged` state frames are unbalanced.
    pub fn has_changed(
        &mut self,
        py: Python<'_>,
        id: CycleId,
        value: Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        if let Some(live) = &self.formatting_context {
            let live = live.bind(py);
            let state = live.call_method1("get", ("forloop",))?;
            let state = if state.is_none() {
                live.getattr("render_context")?
            } else {
                state
            };
            let key = ("rusty_ifchanged", id.0);
            let previous = state.call_method1("get", (key,))?;
            let changed = value.ne(&previous)?;
            if changed {
                state.set_item(key, value)?;
            }
            return Ok(changed);
        }
        let frame = self
            .changed_states
            .last_mut()
            .expect("render has a state frame");
        let changed = match frame.get(&id) {
            Some(previous) => value.ne(previous.bind(py))?,
            None => value.ne(py.None())?,
        };
        if changed {
            frame.insert(id, value.unbind());
        }
        Ok(changed)
    }

    pub fn reset_cycle(&mut self, py: Python<'_>, id: CycleId) -> PyResult<()> {
        if let Some(live) = &self.formatting_context {
            live.bind(py)
                .getattr("render_context")?
                .set_item(("rusty_cycle", id.0), 0)?;
        }
        self.cycle_indices.remove(&id);
        Ok(())
    }

    pub fn insert_cycle(&mut self, key: String, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.cycle_variables.insert(key.clone());
        if let Some(live) = &self.formatting_context {
            // Django's named cycles update the nearest existing binding,
            // including across block/with scopes, rather than shadowing it.
            live.bind(value.py())
                .call_method1("set_upward", (&key, &value))?;
            self.assigned.insert(key.clone());
            self._insert(key, value, true);
            return Ok(());
        }
        if self.get(&key).is_some() {
            self.assigned.insert(key.clone());
            self._insert(key, value, true);
            Ok(())
        } else {
            self.insert(key, value)
        }
    }

    pub fn render_partial(&mut self, py: Python<'_>, body: &Template) -> RenderResult<'static> {
        if let Some(context) = &self.formatting_context {
            let output = py
                .import("django_rusty_templates.inheritance")?
                .getattr("render_partial_body")?
                .call1((Py::new(py, body.clone())?, context.bind(py)))?
                .extract()?;
            return Ok(Cow::Owned(output));
        }
        let previous = std::mem::replace(&mut self.changed_states, vec![HashMap::new()]);
        let output = body
            .render(py, self)
            .map(|output| Cow::Owned(output.into_owned()));
        self.changed_states = previous;
        output
    }

    pub fn merge_render_state(
        &mut self,
        py: Python<'_>,
        scoped: Self,
        local_names: &[String],
    ) -> PyResult<()> {
        for name in &scoped.cycle_variables {
            if !local_names.contains(name)
                && self.has_binding_key(name)
                && let Some(value) = scoped.get(name)
            {
                self.insert_cycle(name.clone(), value.bind(py).clone())?;
            }
        }
        self.cycle_indices = scoped.cycle_indices;
        self.changed_states = scoped.changed_states;
        self.include_cache = scoped.include_cache;
        self.autoescape = scoped.autoescape;
        self.use_l10n = scoped.use_l10n;
        self.use_tz = scoped.use_tz;
        Ok(())
    }

    pub fn next_cycle_index(
        &mut self,
        py: Python<'_>,
        id: CycleId,
        length: usize,
    ) -> PyResult<usize> {
        if let Some(live) = &self.formatting_context {
            let state = live.bind(py).getattr("render_context")?;
            let key = ("rusty_cycle", id.0);
            let index: usize = state.call_method1("get", (key, 0))?.extract()?;
            state.set_item(key, (index + 1) % length)?;
            return Ok(index);
        }
        Ok(*self
            .cycle_indices
            .entry(id)
            .and_modify(|index| *index = (*index + 1) % length)
            .or_default())
    }
}

#[pyclass(mapping, from_py_object)]
#[derive(Clone)]
pub struct PyContext {
    pub context: Gc<Mutex<Context>>,
}

impl PyContext {
    pub fn new(context: Context) -> Self {
        Self {
            context: Gc::new(Mutex::new(context)),
        }
    }
}

#[pymethods]
impl PyContext {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.context.trace(&visit)
    }

    #[getter]
    fn autoescape(&self, py: Python<'_>) -> bool {
        self.context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned")
            .autoescape
    }

    #[getter]
    fn use_l10n(&self, py: Python<'_>) -> Option<bool> {
        self.context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned")
            .use_l10n
    }

    #[getter]
    fn use_tz(&self, py: Python<'_>) -> Option<bool> {
        self.context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned")
            .use_tz
    }

    #[getter]
    fn request<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyAny>> {
        let guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        guard
            .request
            .as_ref()
            .map(|request| request.bind(py).clone())
    }

    #[pyo3(signature = (key, fallback=None))]
    fn get<'py>(
        &self,
        py: Python<'py>,
        key: String,
        fallback: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        match self.__getitem__(py, key) {
            Ok(value) => Ok(value),
            Err(error) if error.is_instance_of::<PyKeyError>(py) => {
                Ok(fallback.unwrap_or_else(|| py.None().into_bound(py)))
            }
            Err(error) => Err(error),
        }
    }

    fn __contains__(&self, py: Python<'_>, key: String) -> bool {
        let guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        guard.get(&key).is_some()
    }

    fn __getitem__<'py>(&self, py: Python<'py>, key: String) -> PyResult<Bound<'py, PyAny>> {
        let guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        match guard.resolve_root(py, &key)? {
            Some(value) if value.is_instance_of::<BlockMarker>() => {
                let marker = value.extract::<PyRef<'_, BlockMarker>>()?.clone();
                Ok(Bound::new(
                    py,
                    super::inheritance::BlockCallback {
                        marker,
                        context: self.context.clone(),
                    },
                )?
                .into_any())
            }
            Some(value) => Ok(value),
            None => Err(PyKeyError::new_err(key)),
        }
    }

    fn __setitem__<'py>(
        &self,
        py: Python<'py>,
        key: String,
        value: Bound<'py, PyAny>,
    ) -> PyResult<()> {
        let mut guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        guard.insert(key, value)
    }
}

#[derive(Debug, IntoPyObject)]
pub enum ContentString<'t> {
    String(Cow<'t, str>),
    HtmlSafe(Cow<'t, str>),
    HtmlUnsafe(Cow<'t, str>),
}

#[allow(clippy::needless_lifetimes)] // https://github.com/rust-lang/rust-clippy/issues/13923
impl<'t, 'py> ContentString<'t> {
    pub fn content(self) -> Cow<'t, str> {
        match self {
            Self::String(content) | Self::HtmlSafe(content) => content,
            Self::HtmlUnsafe(content) => Cow::Owned(encode_quoted_attribute(&content).into_owned()),
        }
    }

    pub fn as_raw(&self) -> &Cow<'t, str> {
        match self {
            Self::String(content) | Self::HtmlSafe(content) | Self::HtmlUnsafe(content) => content,
        }
    }

    pub fn into_raw(self) -> Cow<'t, str> {
        match self {
            Self::String(content) | Self::HtmlSafe(content) | Self::HtmlUnsafe(content) => content,
        }
    }

    pub fn map_content(self, f: impl FnOnce(Cow<'t, str>) -> Cow<'t, str>) -> Content<'t, 'py> {
        Content::String(match self {
            Self::String(content) => Self::String(f(content)),
            Self::HtmlSafe(content) => Self::HtmlSafe(f(content)),
            Self::HtmlUnsafe(content) => Self::HtmlUnsafe(f(content)),
        })
    }
}

fn resolve_python<'t>(value: Bound<'_, PyAny>, context: &Context) -> PyResult<ContentString<'t>> {
    if !context.autoescape {
        return Ok(ContentString::String(
            value.str()?.extract::<String>()?.into(),
        ));
    }
    // Exact built-in strings cannot define __html__. Keep subclasses on the
    // protocol path so SafeString and custom HTML methods retain their behavior.
    if value.is_exact_instance_of::<PyString>() {
        return Ok(ContentString::HtmlUnsafe(value.extract::<String>()?.into()));
    }
    let py = value.py();

    let value = match value.is_instance_of::<PyString>() {
        true => value,
        false => value.str()?.into_any(),
    };
    Ok(
        match value
            .getattr(intern!(py, "__html__"))
            .ok_or_isinstance_of::<PyAttributeError>(py)?
        {
            Ok(html) => ContentString::HtmlSafe(html.call0()?.extract::<String>()?.into()),
            Err(_) => ContentString::HtmlUnsafe(value.str()?.extract::<String>()?.into()),
        },
    )
}

fn resolve_bigint(bigint: BigInt, at: At) -> Result<usize, PyRenderError> {
    match bigint.to_isize() {
        Some(n) => {
            let n = n.max(0);
            #[allow(clippy::cast_sign_loss)]
            let n = n as usize;
            Ok(n)
        }
        None => Err(RenderError::OverflowError {
            argument: bigint.to_string(),
            argument_at: at.into(),
        }
        .into()),
    }
}

#[derive(Debug, IntoPyObject)]
pub enum Content<'t, 'py> {
    Py(Bound<'py, PyAny>),
    String(ContentString<'t>),
    Float(f64),
    Int(BigInt),
    Bool(bool),
}

fn render_value_in_context<'t>(value: ContentString<'t>, context: &Context) -> Cow<'t, str> {
    match value {
        ContentString::String(content) | ContentString::HtmlSafe(content) => content,
        ContentString::HtmlUnsafe(content) => match context.autoescape {
            true => Cow::Owned(encode_quoted_attribute(&content).into_owned()),
            false => Cow::Owned(content.into()),
        },
    }
}

/// Skip the escape writer for plain text: its per-byte bookkeeping is more
/// expensive than scanning once and appending the unchanged slice in bulk.
fn append_escaped(text: &str, output: &mut String) {
    // The library's append writer regresses large values on some platforms.
    // Its Cow encoder still borrows unchanged text, and borrowing the Python
    // input avoids the source copy even when an escaped fragment is required.
    if text.len() >= 1024 {
        output.push_str(&encode_quoted_attribute(text));
        return;
    }
    match text
        .bytes()
        .position(|byte| matches!(byte, b'&' | b'<' | b'>' | b'"' | b'\''))
    {
        Some(first) => {
            output.push_str(&text[..first]);
            encode_quoted_attribute_to_string(&text[first..], output);
        }
        None => output.push_str(text),
    }
}

/// Borrow Python text only while its owning handle is alive. `PyO3`'s `to_cow`
/// preserves the same Unicode conversion and errors as String extraction.
pub(crate) fn append_python_string(
    value: &Bound<'_, PyAny>,
    escape: bool,
    output: &mut String,
) -> PyResult<()> {
    let value = value.cast::<PyString>()?.to_cow()?;
    if escape {
        append_escaped(&value, output);
    } else {
        output.push_str(&value);
    }
    Ok(())
}

impl<'t, 'py> Content<'t, 'py> {
    pub fn render_into(self, context: &Context, output: &mut String) -> PyResult<()> {
        match self {
            Self::Py(content) => {
                let py = content.py();
                if content.is_exact_instance_of::<PyString>() {
                    let autoescape =
                        if context.format_values && context.formatting_context.is_some() {
                            context
                                .django_context(py)?
                                .getattr("autoescape")?
                                .extract()?
                        } else {
                            context.autoescape
                        };
                    return append_python_string(&content, autoescape, output);
                }
                if context.format_values
                    && (context.formatting_context.is_some()
                        || !content.is_instance_of::<PyString>())
                {
                    let rendered = if content.is_instance_of::<PyString>() {
                        let autoescape: bool = context
                            .django_context(py)?
                            .getattr("autoescape")?
                            .extract()?;
                        if autoescape {
                            CONDITIONAL_ESCAPE
                                .import(py, "django.utils.html", "conditional_escape")?
                                .call1((content,))?
                        } else {
                            content.str()?.into_any()
                        }
                    } else {
                        RENDER_VALUE
                            .import(py, "django.template.base", "render_value_in_context")?
                            .call1((content, context.value_formatting_context(py)?))?
                    };
                    return append_python_string(&rendered, false, output);
                }
                if !context.autoescape {
                    return append_python_string(&content.str()?.into_any(), false, output);
                }
                let value = if content.is_instance_of::<PyString>() {
                    content
                } else {
                    content.str()?.into_any()
                };
                match value
                    .getattr(intern!(py, "__html__"))
                    .ok_or_isinstance_of::<PyAttributeError>(py)?
                {
                    Ok(html) => append_python_string(&html.call0()?, false, output),
                    Err(_) => append_python_string(&value.str()?.into_any(), true, output),
                }
            }
            Self::String(content) => {
                match content {
                    ContentString::HtmlUnsafe(text) if context.autoescape => {
                        append_escaped(&text, output);
                    }
                    content => output.push_str(&content.into_raw()),
                }
                Ok(())
            }
            // Numeric formatting retains Django's localization/timezone path.
            value => {
                output.push_str(&value.render(context)?);
                Ok(())
            }
        }
    }

    pub fn render(self, context: &Context) -> PyResult<Cow<'t, str>> {
        let output = match self {
            Self::Py(content)
                if context.format_values
                    && context.formatting_context.is_some()
                    && content.is_exact_instance_of::<PyString>() =>
            {
                let py = content.py();
                let autoescape: bool = context
                    .django_context(py)?
                    .getattr("autoescape")?
                    .extract()?;
                let value: String = content.extract()?;
                return Ok(Cow::Owned(if autoescape {
                    encode_quoted_attribute(&value).into_owned()
                } else {
                    value
                }));
            }
            Self::Py(content)
                if context.format_values
                    && context.formatting_context.is_some()
                    && content.is_instance_of::<PyString>() =>
            {
                let py = content.py();
                let autoescape: bool = context
                    .django_context(py)?
                    .getattr("autoescape")?
                    .extract()?;
                let rendered = if autoescape {
                    CONDITIONAL_ESCAPE
                        .import(py, "django.utils.html", "conditional_escape")?
                        .call1((content,))?
                } else {
                    content.str()?.into_any()
                };
                return Ok(Cow::Owned(rendered.extract()?));
            }
            // Strings need escaping/safety handling, but neither localization
            // nor timezone conversion. Avoid building a Django context for them.
            Self::Py(content)
                if context.format_values
                    && (context.formatting_context.is_some()
                        || !content.is_instance_of::<PyString>()) =>
            {
                let py = content.py();
                let rendered = RENDER_VALUE
                    .import(py, "django.template.base", "render_value_in_context")?
                    .call1((content, context.value_formatting_context(py)?))?;
                return Ok(Cow::Owned(rendered.extract()?));
            }
            Self::Py(content) => resolve_python(content, context)?,
            Self::String(content) => content,
            Self::Float(content) if context.format_values => {
                return Python::attach(|py| {
                    Content::Py(pyo3::types::PyFloat::new(py, content).into_any())
                        .render(context)
                        .map(|output| Cow::Owned(output.into_owned()))
                });
            }
            Self::Float(content) => return Ok(content.to_string().into()),
            Self::Int(content) => return Ok(content.to_string().into()),
            Self::Bool(true) => return Ok(Cow::Borrowed("True")),
            Self::Bool(false) => return Ok(Cow::Borrowed("False")),
        };
        Ok(render_value_in_context(output, context))
    }

    pub fn resolve_string(self, context: &Context) -> PyResult<ContentString<'t>> {
        Ok(match self {
            Self::String(content) => content,
            Self::Float(content) => ContentString::String(content.to_string().into()),
            Self::Int(content) => ContentString::String(content.to_string().into()),
            Self::Py(content) => return resolve_python(content, context),
            Self::Bool(true) => ContentString::String(Cow::Borrowed("True")),
            Self::Bool(false) => ContentString::String(Cow::Borrowed("False")),
        })
    }
    /// Resolve to a string, raising an error if the content is not a string or a string-like Python object
    pub fn resolve_string_strict(
        self,
        context: &Context,
        argument_at: SourceSpan,
    ) -> Result<ContentString<'t>, PyRenderError> {
        match self {
            Self::String(content) => Ok(content),
            Self::Py(content) if content.is_instance_of::<PyString>() => {
                Ok(resolve_python(content, context)?)
            }
            _ => Err(RenderError::InvalidArgumentString { argument_at }.into()),
        }
    }

    #[expect(
        clippy::missing_panics_doc,
        reason = "int() always returns a Python int, which converts to BigInt"
    )]
    pub fn to_bigint(&self) -> Option<BigInt> {
        match self {
            Self::Int(left) => Some(left.clone()),
            Self::String(left) => left.as_raw().parse::<BigInt>().ok(),
            Self::Float(left) => left.trunc().to_bigint(),
            Self::Py(left) => match left.extract::<BigInt>() {
                Ok(left) => Some(left),
                Err(_) => {
                    let int = PyType::new::<PyInt>(left.py());
                    let left = int.call1((left,)).ok()?;
                    Some(
                        left.extract::<BigInt>()
                            .expect("Python integers are BigInt compatible"),
                    )
                }
            },
            Self::Bool(true) => 1.to_bigint(),
            Self::Bool(false) => 0.to_bigint(),
        }
    }

    /// Convert Content to usize, providing detailed errors if the conversion fails
    pub fn resolve_usize(self, argument_at: At) -> Result<usize, PyRenderError> {
        match self {
            Self::Int(n) => resolve_bigint(n, argument_at),
            Self::String(s) => match s.as_raw().parse::<BigInt>() {
                Ok(n) => resolve_bigint(n, argument_at),
                Err(_) => Err(RenderError::InvalidArgumentInteger {
                    argument: format!("'{}'", s.as_raw()),
                    argument_at: argument_at.into(),
                }
                .into()),
            },
            Self::Float(f) => match f.trunc().to_bigint() {
                Some(n) => resolve_bigint(n, argument_at),
                None => Err(RenderError::InvalidArgumentFloat {
                    argument: f.to_string(),
                    argument_at: argument_at.into(),
                }
                .into()),
            },
            Self::Py(obj) => match obj.extract::<BigInt>() {
                Ok(n) => resolve_bigint(n, argument_at),
                Err(_) => {
                    let argument = obj.to_string();
                    let argument_at = argument_at.into();
                    match obj.extract::<f64>() {
                        Ok(_) => Err(RenderError::InvalidArgumentFloat {
                            argument,
                            argument_at,
                        }
                        .into()),
                        Err(_) => Err(RenderError::InvalidArgumentInteger {
                            argument,
                            argument_at,
                        }
                        .into()),
                    }
                }
            },
            Self::Bool(true) => Ok(1),
            Self::Bool(false) => Ok(0),
        }
    }
    pub fn to_bool(&self) -> PyResult<bool> {
        Ok(match self {
            Self::Bool(b) => *b,
            Self::Int(n) => !n.is_zero(),
            Self::Float(f) => !f.is_zero(),
            Self::String(s) => !s.as_raw().is_empty(),
            Self::Py(obj) => obj.is_truthy()?,
        })
    }

    #[expect(
        clippy::missing_panics_doc,
        reason = "converting Rust numbers to Python objects cannot fail"
    )]
    pub fn to_py(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        match self {
            Self::Py(object) => object.clone(),
            Self::Int(i) => i
                .into_pyobject(py)
                .expect("A BigInt can always be converted to a Python int.")
                .into_any(),
            Self::Float(f) => f
                .into_pyobject(py)
                .expect("An f64 can always be converted to a Python float.")
                .into_any(),
            Self::String(s) => match s {
                ContentString::String(s) | ContentString::HtmlUnsafe(s) => s
                    .into_pyobject(py)
                    .expect("A string can always be converted to a Python str.")
                    .into_any(),
                ContentString::HtmlSafe(s) => {
                    let string = s
                        .into_pyobject(py)
                        .expect("A string can always be converted to a Python str.");
                    let mark_safe = MARK_SAFE
                        .import(py, "django.utils.safestring", "mark_safe")
                        .expect("Should be able to import `django.utils.safestring.mark_safe`");
                    mark_safe
                        .call1((string,))
                        .expect("`mark_safe` should not raise if given a string")
                }
            },
            Self::Bool(b) => PyBool::new(py, *b).to_owned().into_any(),
        }
    }
}
pub trait IntoOwnedContent<'t, 'py> {
    fn into_content(self) -> Content<'t, 'py>;
}

pub trait AsBorrowedContent<'a, 't, 'py>
where
    'a: 't,
{
    fn as_content(&'a self) -> Content<'t, 'py>;
}

impl<'a, 't, 'py> AsBorrowedContent<'a, 't, 'py> for str
where
    'a: 't,
{
    fn as_content(&'a self) -> Content<'t, 'py> {
        Content::String(ContentString::String(Cow::Borrowed(self)))
    }
}

impl<'t, 'py> IntoOwnedContent<'t, 'py> for String {
    fn into_content(self) -> Content<'t, 'py> {
        Content::String(ContentString::String(Cow::Owned(self)))
    }
}

impl<'t, 'py> IntoOwnedContent<'t, 'py> for Cow<'t, str> {
    fn into_content(self) -> Content<'t, 'py> {
        Content::String(ContentString::String(self))
    }
}
