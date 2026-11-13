use crate::gc::{Gc, GcOnceLock, Trace};
use pyo3::{PyTraverseError, PyVisit};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;
use std::iter::zip;
use std::sync::{Arc, Mutex, OnceLock};

use html_escape::{encode_quoted_attribute, encode_quoted_attribute_to_string};
use miette::SourceSpan;
use num_bigint::{BigInt, ToBigInt};
use num_traits::{ToPrimitive, Zero};
use pyo3::exceptions::{PyAttributeError, PyKeyError, PyTypeError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::{MutexExt, PyOnceLock};
use pyo3::types::{PyBool, PyDict, PyInt, PyList, PyString, PyTuple, PyType};
use rustc_hash::{FxBuildHasher, FxHashMap};

use super::common::interned_name;
use super::dateformat::localize_datetime;
use super::filters::FileSizeStrings;
use super::filters::FloatNumberFormats;
use super::filters::TruncationText;
use super::filters::python_float_str;
use super::humanize::HumanizeStrings;
use super::inheritance::{BlockEntry, BlockMarker};
use super::l10n::{NumberFormat, localize_int, localize_number};
use super::language_cache::{LanguageCache, LanguageInfo};
use super::stack::RenderStack;
use super::timesince::TimeStrings;
use crate::error::{AnnotatePyErr, PyRenderError, RenderError};
use crate::parse::CycleId;
use crate::render::RenderResult;
use crate::render::dateformat::DateCache;
use crate::template::django_rusty_templates::{Engine, Template, get_template, select_template};
use crate::utils::PyResultMethods;
use dtl_lexer::types::{At, TemplateString};

static DJANGO_CONTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static RENDER_VALUE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static LOCALIZABLE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Whether Django's `render_value_in_context` would do more than `str()`
/// and escaping: `localize` formats numbers, dates and times (and bools,
/// as `str()`), and `template_localtime` only converts datetimes. Anything
/// else, such as `None`, a list or a model instance, renders natively.
fn is_localizable(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    let py = value.py();
    let types = LOCALIZABLE.get_or_try_init(py, || -> PyResult<_> {
        let decimal = py.import("decimal")?.getattr("Decimal")?;
        let datetime = py.import("datetime")?;
        Ok(pyo3::types::PyTuple::new(
            py,
            [
                py.get_type::<PyInt>().into_any(),
                py.get_type::<pyo3::types::PyFloat>().into_any(),
                decimal,
                datetime.getattr("date")?,
                datetime.getattr("time")?,
            ],
        )?
        .into_any()
        .unbind())
    })?;
    value.is_instance(types.bind(py))
}
static CONDITIONAL_ESCAPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

static SAFE_STRING: PyOnceLock<Py<PyType>> = PyOnceLock::new();

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
    /// How many values `forloop` had as a variable when the loop began.
    /// More than this means something in the body rebound the name.
    forloop_values: usize,
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

/// Hashed on every variable read and loop iteration. Template variable names
/// are short and come from template source, so a fast hash beats `SipHash`.
type BindingMap = FxHashMap<String, Vec<Py<PyAny>>>;

impl Trace for ForLoop {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.dictionary.trace(visit)
    }
}

/// A live render's Django `Context`, set aside while a callback-free loop
/// renders natively. Python can't see the loop's scopes, so names bound since
/// `names`/`loops` were the depths resolve natively; every other name is read
/// from the live context, where callbacks may have changed it.
#[derive(Debug)]
struct Shadow {
    live: Py<PyAny>,
    names: usize,
    loops: usize,
}

/// Whether the filters of the expression being resolved still apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FilterChain {
    #[default]
    Run,
    Skip,
}

/// Whether a render ever bound `forloop` as a variable. Until one does,
/// the name inside a loop can only be the loop object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ForloopName {
    #[default]
    Unbound,
    Bound,
}

#[derive(Debug, Default)]
pub struct Context {
    context: BindingMap,
    // The caller's context dict, read on lookup instead of being copied in.
    // A name is copied into `context` before its first write, so a name in
    // `context` always hides the same name here.
    source: Option<Py<PyDict>>,
    loops: Vec<ForLoop>,
    pub request: Option<Py<PyAny>>,
    pub autoescape: bool,
    pub use_l10n: Option<bool>,
    pub use_tz: Option<bool>,
    // Borrowed mutably while formatting; read through `render_into`'s `&self`.
    pub(crate) date_cache: RefCell<DateCache>,
    pub format_values: bool,
    pub instrument_render: bool,
    pub formatting_context: Option<Py<PyAny>>,
    // Set while a callback-free loop of a live render runs on native scopes.
    shadow: Option<Shadow>,
    // For a live context this render created, how many references Python
    // might use to reach it once installed (`live::outside_references`).
    // While there are no more, only this render can reach it.
    pub(crate) live_refs: Option<isize>,
    live_methods: Cell<Option<super::live::Methods>>,
    // Number formats of the active language, read on the first localized
    // number. `{% language %}` replaces it for its body. It holds only Rust
    // strings, so `Trace` has nothing to visit. Child scopes share the cell,
    // so a number printed in a `{% with %}` body fills it for the parent and
    // later scopes rather than reading the formats again in each scope.
    pub(crate) number_format: Arc<OnceLock<Option<NumberFormat>>>,
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
    /// The scope depth at each `takes_context` call in progress: a tag
    /// can't pop below it.
    callback_floors: Vec<usize>,
    /// Django's `RenderContext` for this render, made when a tag asks.
    render_context: Option<Py<PyAny>>,
    /// `Skip` while a filter chain unwinds from a missing variable that
    /// `string_if_invalid` replaces; see `Filter::resolve`.
    pub(crate) filter_chain: FilterChain,
    /// Names first bound by a scoped write, with how many values that write
    /// found: a copy of the caller's value, or none.
    scope_bases: FxHashMap<String, usize>,
    /// Whether anything ever bound the name `forloop` as a variable.
    forloop_name: ForloopName,
    cycle_variables: HashSet<String>,
    // Created on first use: most renders never need them, and a smaller
    // `Context` is cheaper to move into and out of Python callbacks.
    caches: Option<Box<LanguageCaches>>,
    // How deep in nested templates this render is. Child contexts copy it.
    pub(crate) render_stack: RenderStack,
}

/// Translated strings and formats a render keeps for its active language.
#[derive(Clone, Debug, Default)]
pub(crate) struct LanguageCaches {
    pub(crate) float_number_format: LanguageCache<FloatNumberFormats>,
    pub(crate) truncation_text: LanguageCache<TruncationText>,
    /// `yesno`'s translated default, "yes,no,maybe".
    pub(crate) yesno_default: LanguageCache<String>,
    /// Whether `urlize` can link natively, checked once per render.
    pub(crate) urlize_rules: crate::render::urlize::NativeLinks,
    pub(crate) time_strings: LanguageCache<TimeStrings>,
    pub(crate) file_size_strings: LanguageCache<FileSizeStrings>,
    pub(crate) humanize_strings: LanguageCache<HumanizeStrings>,
    // `get_language_info(code)` results, which don't depend on the language.
    pub(crate) language_info: HashMap<String, LanguageInfo>,
    // `language_name_translated` results for the active language, by code.
    pub(crate) translated_language_names: LanguageCache<HashMap<String, String>>,
}

impl Context {
    /// This render's language caches, created on first use.
    pub(crate) fn caches(&mut self) -> &mut LanguageCaches {
        self.caches.get_or_insert_with(Box::default)
    }
}

impl Trace for Context {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.context.trace(visit)?;
        self.source.trace(visit)?;
        self.render_context.trace(visit)?;
        self.loops.trace(visit)?;
        self.request.trace(visit)?;
        self.formatting_context.trace(visit)?;
        if let Some(shadow) = &self.shadow {
            shadow.live.trace(visit)?;
        }
        // A collection can run while a format call holds the cache.
        if let Ok(date_cache) = self.date_cache.try_borrow() {
            date_cache.trace(visit)?;
        }
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
    /// A context with `context`'s bindings. A later pair for the same name
    /// replaces an earlier one.
    pub fn new(
        context: impl IntoIterator<Item = (String, Py<PyAny>)>,
        request: Option<Py<PyAny>>,
        autoescape: bool,
    ) -> Self {
        let context = context.into_iter();
        let mut bindings =
            BindingMap::with_capacity_and_hasher(context.size_hint().0, FxBuildHasher);
        for (key, value) in context {
            bindings.insert(key, vec![value]);
        }
        let context = bindings;
        Self {
            forloop_name: ForloopName::Unbound,
            request,
            context,
            source: None,
            autoescape,
            use_l10n: None,
            use_tz: None,
            date_cache: RefCell::default(),
            format_values: false,
            instrument_render: true,
            formatting_context: None,
            shadow: None,
            live_refs: None,
            live_methods: Cell::new(None),
            number_format: Arc::default(),
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
            callback_floors: Vec::new(),
            render_context: None,
            filter_chain: FilterChain::Run,
            scope_bases: FxHashMap::default(),
            cycle_variables: HashSet::new(),
            caches: None,
            render_stack: RenderStack::default(),
        }
    }

    /// Reads `source`'s bindings lazily. They take precedence over the
    /// bindings given to `new`, as later pairs there do.
    #[must_use]
    pub fn with_source(mut self, source: &Bound<'_, PyDict>) -> Self {
        let py = source.py();
        self.context.retain(|key, _| {
            // The built-in names are checked on every render; interned
            // strings spare allocating and hashing each one.
            let shadowed = match key.as_str() {
                "None" => source.contains(intern!(py, "None")),
                "True" => source.contains(intern!(py, "True")),
                "False" => source.contains(intern!(py, "False")),
                key => source.contains(key),
            };
            !shadowed.unwrap_or(false)
        });
        self.source = Some(source.clone().unbind());
        self
    }

    /// Whether `input`, a Django `Context`, already shows exactly what this
    /// context holds: its dicts are the built-in names and this context's
    /// source, and nothing native shadows or adds to them. A callback can then
    /// see `input` as it is, without a snapshot copied into it and back.
    pub(crate) fn is_live_view_of(&self, py: Python<'_>, dicts: &Bound<'_, PyList>) -> bool {
        let Some(source) = &self.source else {
            return false;
        };
        dicts.len() == 2
            && dicts.get_item(1).is_ok_and(|dict| dict.is(source.bind(py)))
            && self.loops.is_empty()
            && self.names.is_empty()
            && self.assigned.is_empty()
            && self.cycle_variables.is_empty()
            && self.django_context_changes.is_empty()
            && self
                .context
                .keys()
                .all(|key| matches!(key.as_str(), "None" | "True" | "False"))
    }

    /// Whether the caller's dict has `key`.
    fn source_contains(&self, key: &str) -> bool {
        self.source.as_ref().is_some_and(|source| {
            Python::attach(|py| source.bind(py).contains(key).unwrap_or(false))
        })
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
            source: self.source.as_ref().map(|source| source.clone_ref(py)),
            autoescape: self.autoescape,
            use_l10n: self.use_l10n,
            use_tz: self.use_tz,
            date_cache: RefCell::default(),
            format_values: self.format_values,
            instrument_render: self.instrument_render,
            formatting_context: self.formatting_context.as_ref().map(|v| v.clone_ref(py)),
            shadow: self.shadow.as_ref().map(|shadow| Shadow {
                live: shadow.live.clone_ref(py),
                names: shadow.names,
                loops: shadow.loops,
            }),
            live_refs: None,
            live_methods: self.live_methods.clone(),
            number_format: Arc::clone(&self.number_format),
            callback_input: self.callback_input.as_ref().map(|v| v.clone_ref(py)),
            inheritance_blocks: self.inheritance_blocks.clone(),
            python_blocks: self.python_blocks.as_ref().map(|v| v.clone_ref(py)),
            current_template: self.current_template.as_ref().map(|v| v.clone_ref(py)),
            root_template: self.root_template.as_ref().map(|v| v.clone_ref(py)),
            inheritance_history: self
                .inheritance_history
                .iter()
                .map(|v| v.clone_ref(py))
                .collect(),
            django_context_base: RefCell::new(None),
            django_context_changes: HashSet::new(),
            string_if_invalid: self.string_if_invalid.clone(),
            loops: self
                .loops
                .iter()
                .map(|loop_| ForLoop {
                    forloop_values: loop_.forloop_values,
                    count: loop_.count,
                    len: loop_.len,
                    dictionary: loop_.dictionary.clone_ref(py),
                })
                .collect(),
            names: self.names.clone(),
            forloop_name: self.forloop_name,
            include_cache: self.include_cache.clone(),
            cycle_indices: self.cycle_indices.clone(),
            assigned: self.assigned.clone(),
            callback_floors: self.callback_floors.clone(),
            render_context: self.render_context.as_ref().map(|v| v.clone_ref(py)),
            filter_chain: FilterChain::Run,
            scope_bases: self.scope_bases.clone(),
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
            caches: self.caches.clone(),
            render_stack: self.render_stack,
        }
    }

    /// Hand the render's caches (date formats, translated strings) to a
    /// context that renders in this one's place, with the same language and
    /// time zone, so what it looks up is still there afterwards. The other
    /// context hands them back the same way.
    pub(crate) fn move_caches_to(&mut self, other: &mut Self) {
        other.date_cache = std::mem::take(&mut self.date_cache);
        other.caches = self.caches.take();
    }

    fn visible_bindings(&self) -> impl Iterator<Item = (&String, &Vec<Py<PyAny>>)> {
        self.context.iter()
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
        let dict = match &self.source {
            Some(source) => source.bind(py).copy()?,
            None => PyDict::new(py),
        };
        for (key, values) in self.visible_bindings() {
            if let Some(value) = values.last() {
                dict.set_item(key, value)?;
            } else if self.source.is_some() && dict.contains(key)? {
                dict.del_item(key)?;
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
                Ok(value) => self._insert(&name, value, true),
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
            if let Some(value) = self.get(py, key) {
                target.set_item(key, &value)?;
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
            if let Some(value) = self.get(py, name) {
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
            cached_base.insert(self.as_dict(py)?.unbind())
        }
        .bind(py);
        let overlay = PyDict::new(py);
        if !self.loops.is_empty() {
            overlay.set_item("forloop", self.loop_dictionary(py, 0)?)?;
        }
        if let Some(value) = self.get(py, "block")
            && let Ok(marker) = value.extract::<PyRef<'_, BlockMarker>>()
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
            if let Some(value) = self.get(py, key) {
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

    pub fn get<'py>(&self, py: Python<'py>, key: &str) -> Option<Bound<'py, PyAny>> {
        if let Some(values) = self.context.get(key) {
            return values.last().map(|value| value.bind(py).clone());
        }
        let source = self.source.as_ref()?.bind(py);
        source.get_item(interned_name(py, key)).ok().flatten()
    }

    pub fn resolve_root<'py>(
        &self,
        py: Python<'py>,
        key: &str,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        if let Some(context) = &self.formatting_context {
            let context = context.bind(py);
            let methods = super::live::Methods::cached(&self.live_methods, context)?;
            return super::live::get(context, methods, &interned_name(py, key));
        }
        if let Some(shadow) = &self.shadow {
            if key == "forloop" && !self.forloop_rebound() {
                if self.loops.len() > shadow.loops {
                    return self.loop_dictionary(py, 0).map(Some);
                }
            } else if self.names[shadow.names..]
                .iter()
                .any(|names| names.contains(key))
            {
                return Ok(self.get(py, key));
            }
            let live = shadow.live.bind(py);
            let methods = super::live::Methods::cached(&self.live_methods, live)?;
            return super::live::get(live, methods, &interned_name(py, key));
        }
        if key == "forloop" && !self.loops.is_empty() && !self.forloop_rebound() {
            return self.loop_dictionary(py, 0).map(Some);
        }
        Ok(self.get(py, key))
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

    /// Render `render` on native scopes over this live render's Django
    /// `Context`, for nodes that never hand the context to Python. Returns
    /// `None` without calling `render` when Python might still reach the
    /// context, so the caller renders on the live context as usual.
    pub(crate) fn shadowed<T>(
        &mut self,
        py: Python<'_>,
        render: impl FnOnce(&mut Self) -> T,
    ) -> PyResult<Option<T>> {
        let Some(live) = self.formatting_context.take_if(|live| {
            Self::unreachable_from_python(py, live, self.live_refs).unwrap_or(false)
        }) else {
            return Ok(None);
        };
        if let Err(error) = self.refresh_live_flags(py, &live) {
            self.formatting_context = Some(live);
            return Err(error);
        }
        self.shadow = Some(Shadow {
            live,
            names: self.names.len(),
            loops: self.loops.len(),
        });
        let result = render(self);
        let shadow = self.shadow.take().expect("shadow set above");
        self.formatting_context = Some(shadow.live);
        Ok(Some(result))
    }

    /// Whether only this render holds `live`, so Python code that runs
    /// meanwhile (a callable value, `__str__`, a filter) cannot reach it.
    fn unreachable_from_python(
        py: Python<'_>,
        live: &Py<PyAny>,
        live_refs: Option<isize>,
    ) -> PyResult<bool> {
        let Some(live_refs) = live_refs else {
            return Ok(false);
        };
        Ok(super::live::outside_references(live.bind(py))?.is_some_and(|refs| refs <= live_refs))
    }

    fn refresh_live_flags(&mut self, py: Python<'_>, live: &Py<PyAny>) -> PyResult<()> {
        (self.autoescape, self.use_l10n, self.use_tz) = super::live::flags(live.bind(py))?;
        Ok(())
    }

    /// Whether `forloop` at `depth` (or the whole `{{ forloop }}`) belongs to
    /// a loop outside the shadow, so it has to be read as a dict.
    pub(crate) fn shadowed_loop(&self, depth: usize, whole: bool) -> bool {
        self.shadow
            .as_ref()
            .is_some_and(|shadow| whole || self.loops.len() <= shadow.loops + depth)
    }

    pub fn refresh_formatting_flags(&mut self, py: Python<'_>) -> PyResult<()> {
        if let Some(context) = &self.formatting_context {
            (self.autoescape, self.use_l10n, self.use_tz) = super::live::flags(context.bind(py))?;
        }
        Ok(())
    }

    pub fn display(&self, py: Python<'_>) -> String {
        let mut context: BTreeMap<String, Bound<'_, PyAny>> = BTreeMap::new();
        if let Some(source) = &self.source {
            for (key, value) in source.bind(py) {
                if let Ok(key) = key.extract::<String>() {
                    context.insert(key, value);
                }
            }
        }
        for (key, values) in self.visible_bindings() {
            match values.last() {
                Some(value) => context.insert(key.clone(), value.bind(py).clone()),
                None => context.remove(key),
            };
        }
        format!("{context:?}")
    }

    /// `key` is borrowed so a loop rebinding the same name on every
    /// iteration allocates the `String` once, on the first insert.
    fn _insert(&mut self, key: &str, value: Bound<'_, PyAny>, replace: bool) {
        // Pure native renders never need a Python snapshot. Start tracking
        // mutations only after the first conversion has captured a base.
        if self.django_context_base.get_mut().is_some() {
            self.django_context_changes.insert(key.to_string());
        }
        if key == "forloop" {
            self.forloop_name = ForloopName::Bound;
        }
        let py = value.py();
        let value = value.unbind();
        if let Some(values) = self.context.get_mut(key) {
            if replace {
                values.pop();
            }
            let left = values.len();
            values.push(value);
            // The write replaced the copy of the caller's value.
            if self.scope_bases.get(key).is_some_and(|base| left < *base) {
                self.scope_bases.remove(key);
            }
            return;
        }
        // First write to this name. A scoped write keeps a copy of the
        // caller's value below the new one, so the name still resolves if a
        // later write replaces only the top.
        let mut values = Vec::new();
        if !replace {
            if let Some(source) = &self.source
                && let Some(value) = source.bind(py).get_item(key).ok().flatten()
            {
                values.push(value.unbind());
            }
            self.scope_bases.insert(key.to_string(), values.len());
        }
        values.push(value);
        self.context.insert(key.to_string(), values);
    }

    pub fn append(&mut self, key: String, value: Bound<'_, PyAny>) {
        self._insert(&key, value, false);
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
        self._insert(&key, value, replace);
        Ok(())
    }

    pub(crate) fn clear_variable(&mut self, key: &str) {
        if let Some(values) = self.context.get_mut(key) {
            values.clear();
        } else if self.source_contains(key) {
            // Hide the caller's value.
            self.context.insert(key.to_string(), Vec::new());
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
        self._insert(&name, value, replace);
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
        if names.len() == 1 {
            self._insert(&names[0], values, replace);
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
                    self._insert(name, value, replace);
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
        // Open the scope only once its names are bound, so a failed first
        // unpacking leaves nothing for the loop's cleanup to pop.
        if !replace {
            self.names.push(names.iter().cloned().collect());
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
            .context
            .get_mut(name)
            .expect("Variable should have been pushed before");
        values.pop();
        // Back to what the first scoped write found, so the name is the
        // caller's again: their dict may have changed since.
        if self.scope_bases.get(name) == Some(&values.len()) {
            self.scope_bases.remove(name);
            self.context.remove(name);
        }
    }

    pub(crate) fn push_scope(&mut self, names: &[&str]) {
        self.names
            .push(names.iter().map(|name| (*name).to_string()).collect());
    }

    /// Open a scope that binds `values`, as Django's
    /// `context.push(**values)` does. `pop_variables` closes it, and takes
    /// with it whatever the body assigns in the meantime. The scope is open
    /// even when this fails.
    pub(crate) fn push_bindings<'a, 'py>(
        &mut self,
        py: Python<'py>,
        values: impl IntoIterator<Item = (&'a str, Bound<'py, PyAny>)>,
    ) -> PyResult<()> {
        self.names.push(HashSet::new());
        for (name, value) in values {
            if let Some(live) = &self.formatting_context {
                live.bind(py).set_item(name, &value)?;
            }
            // A name given twice keeps its last value.
            let first = self
                .names
                .last_mut()
                .expect("the scope was just opened")
                .insert(name.to_string());
            self._insert(name, value, !first);
        }
        Ok(())
    }

    /// A `takes_context` tag is about to get this context.
    pub(crate) fn enter_callback(&mut self) {
        self.callback_floors.push(self.names.len());
    }

    /// The tag returned. A scope it pushed and never popped stays on the
    /// stack, as the layer does in Django: the next scope to close, the
    /// loop or `{% with %}` around the tag, pops it in place of its own.
    pub(crate) fn leave_callback(&mut self) {
        self.callback_floors.pop();
    }

    /// Whether a `takes_context` tag has a scope of its own to pop.
    fn callback_scope_open(&self) -> bool {
        self.callback_floors
            .last()
            .is_some_and(|floor| self.names.len() > *floor)
    }

    /// Django's `Context.set_upward`: rebind the name where it is bound,
    /// or bind it in the current scope.
    fn set_upward(&mut self, key: String, value: Bound<'_, PyAny>) -> PyResult<()> {
        if self.get(value.py(), &key).is_some() {
            self._insert(&key, value, true);
            Ok(())
        } else {
            self.insert(key, value)
        }
    }

    pub fn pop_variables(&mut self) {
        if let Some(names) = self.names.pop() {
            for name in names {
                self.pop_variable(&name);
            }
        }
    }

    /// Close the newest scope, if one was opened since `scope_depth`
    /// returned `depth`. A tag that fails part-way may not have opened its
    /// scope.
    pub fn pop_scope_above(&mut self, depth: usize) {
        if self.names.len() > depth {
            self.pop_variables();
        }
    }

    /// How many variable scopes are open, for `pop_scope_above`.
    pub fn scope_depth(&self) -> usize {
        self.names.len()
    }

    pub fn push_for_loop(&mut self, py: Python<'_>, len: usize) {
        let forloop_values = self.forloop_values();
        self.loops.push(ForLoop {
            forloop_values,
            count: 0,
            len,
            dictionary: Gc::new_in(py, GcOnceLock::new()),
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

    fn forloop_values(&self) -> usize {
        if self.forloop_name == ForloopName::Unbound {
            return 0;
        }
        self.context.get("forloop").map_or(0, Vec::len)
    }

    /// Whether the innermost loop's body bound `forloop` itself, as a loop
    /// variable or an assignment. The name is then that variable, as in
    /// Django, where the binding replaces the loop object in the same dict.
    pub(crate) fn forloop_rebound(&self) -> bool {
        self.forloop_name == ForloopName::Bound
            && self
                .loops
                .last()
                .is_some_and(|loop_| self.forloop_values() > loop_.forloop_values)
    }

    /// The variable `forloop` that the loop at `depth` found bound inside
    /// the loop around it. Django makes that the loop's `parentloop`.
    fn variable_parent<'py>(&self, py: Python<'py>, depth: usize) -> Option<Bound<'py, PyAny>> {
        if self.forloop_name == ForloopName::Unbound {
            return None;
        }
        let values = self.get_for_loop(depth)?.forloop_values;
        let outer = self.get_for_loop(depth + 1)?.forloop_values;
        if values <= outer {
            return None;
        }
        let value = self.context.get("forloop")?.get(values - 1)?;
        Some(value.bind(py).clone())
    }

    /// The variable `forloop` as the outermost loop found it.
    pub(crate) fn forloop_before_loops<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyAny>> {
        let values = self.loops.first()?.forloop_values;
        match values.checked_sub(1) {
            Some(index) => Some(self.context.get("forloop")?.get(index)?.bind(py).clone()),
            None if self.forloop_name == ForloopName::Bound => None,
            None => self.get(py, "forloop"),
        }
    }

    /// Whether any of the `count` innermost loops has a variable for its
    /// `parentloop`, so the loop can't be read from the native counters.
    pub(crate) fn forloop_parent_is_variable(&self, count: usize) -> bool {
        self.forloop_name == ForloopName::Bound
            && (0..count).any(|depth| {
                let Some(inner) = self.get_for_loop(depth) else {
                    return false;
                };
                self.get_for_loop(depth + 1)
                    .is_some_and(|outer| inner.forloop_values > outer.forloop_values)
            })
    }

    pub fn get_for_loop(&self, depth: usize) -> Option<&ForLoop> {
        let index = self.loops.len().checked_sub(depth + 1)?;
        self.loops.get(index)
    }

    pub fn loop_dictionary_cached(&self, _py: Python<'_>, depth: usize) -> bool {
        self.get_for_loop(depth)
            .is_some_and(|loop_| loop_.dictionary.get().is_some())
    }

    fn fill_loop_dictionary(loop_: &ForLoop, dict: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = dict.py();
        dict.set_item(intern!(py, "counter0"), loop_.counter0())?;
        dict.set_item(intern!(py, "counter"), loop_.counter())?;
        dict.set_item(intern!(py, "revcounter"), loop_.rev_counter())?;
        dict.set_item(intern!(py, "revcounter0"), loop_.rev_counter0())?;
        dict.set_item(intern!(py, "first"), loop_.first())?;
        dict.set_item(intern!(py, "last"), loop_.last())?;
        Ok(())
    }

    pub fn loop_dictionary<'py>(
        &self,
        py: Python<'py>,
        depth: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        if let Some(shadow) = &self.shadow
            && self.loops.len() <= shadow.loops + depth
        {
            // The innermost loop outside the shadow is the live `forloop`.
            let live = shadow.live.bind(py);
            let methods = super::live::Methods::cached(&self.live_methods, live)?;
            return Ok(super::live::get(live, methods, intern!(py, "forloop"))?
                .unwrap_or_else(|| PyDict::new(py).into_any()));
        }
        let Some(loop_) = self.get_for_loop(depth) else {
            return Ok(self
                .get(py, "forloop")
                .unwrap_or_else(|| PyDict::new(py).into_any()));
        };
        loop_
            .dictionary
            .get_or_try_init(py, || {
                let dict = PyDict::new(py);
                // Django's key order: parentloop, length (6.0+), then counters.
                let parent = match self.variable_parent(py, depth) {
                    Some(parent) => parent,
                    None => self.loop_dictionary(py, depth + 1)?,
                };
                dict.set_item("parentloop", parent)?;
                if self.django_series(py)? >= (6, 0) {
                    dict.set_item("length", loop_.length())?;
                }
                Self::fill_loop_dictionary(loop_, dict.as_any())?;
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
                Self::fill_loop_dictionary(loop_, dict.bind(py).as_any())?;
            }
            return Ok(());
        };
        let context = context.bind(py);
        let methods = super::live::Methods::cached(&self.live_methods, context)?;
        if let Some(names) = self.names.last() {
            for key in names {
                if let Some(value) = self.get(py, key) {
                    super::live::set(context, methods, &interned_name(py, key), &value)?;
                }
            }
        }
        let forloop = intern!(py, "forloop");
        let for_loop = self.loops.last().expect("inside native loop");
        let dict = if first {
            let parent = super::live::get(context, methods, forloop)
                .ok()
                .flatten()
                .unwrap_or_else(|| PyDict::new(py).into_any());
            let dict = PyDict::new(py);
            dict.set_item("parentloop", parent)?;
            if self.django_series(py)? >= (6, 0) {
                dict.set_item("length", for_loop.length())?;
            }
            super::live::set(context, methods, forloop, &dict)?;
            dict.into_any()
        } else {
            super::live::get(context, methods, forloop)?
                .ok_or_else(|| PyKeyError::new_err("forloop"))?
        };
        Self::fill_loop_dictionary(for_loop, &dict)
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
            self._insert(&key, value, true);
            return Ok(());
        }
        if self.get(value.py(), &key).is_some() {
            self.assigned.insert(key.clone());
            self._insert(&key, value, true);
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

    fn __contains__(&self, py: Python<'_>, key: String) -> PyResult<bool> {
        let guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        // As `__getitem__` resolves it, so `forloop` is found inside a loop.
        Ok(guard.resolve_root(py, &key)?.is_some())
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
        // Keep the replaced value alive until the lock is released, so its
        // `__del__` cannot re-enter this context while it is locked.
        let replaced = guard.get(py, &key);
        let result = guard.insert(key, value);
        drop(guard);
        drop(replaced);
        result
    }

    /// Django's `Context.update()`: a new scope holding the mapping's values.
    /// The result is a context manager; without one, the scope lasts as
    /// long as the scope the tag was called in.
    fn update<'py>(
        &self,
        py: Python<'py>,
        other_dict: Bound<'py, PyAny>,
    ) -> PyResult<PyContextScope> {
        if !other_dict.hasattr("__getitem__")? {
            return Err(PyTypeError::new_err(
                "other_dict must be a mapping (dictionary-like) object.",
            ));
        }
        let other_dict = if other_dict.hasattr("dicts")? {
            other_dict.getattr("dicts")?.get_item(-1)?
        } else {
            other_dict
        };
        let scope = self.open_scope(py);
        for item in other_dict.call_method0("items")?.try_iter()? {
            let (key, value): (String, Bound<'py, PyAny>) = item?.extract()?;
            self.__setitem__(py, key, value)?;
        }
        Ok(scope)
    }

    /// Django's `Context.push()`: a new scope, holding any mappings and
    /// keyword values given. The result is a context manager.
    #[pyo3(signature = (*args, **kwargs))]
    fn push<'py>(
        &self,
        py: Python<'py>,
        args: &Bound<'py, PyTuple>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<PyContextScope> {
        let scope = self.open_scope(py);
        let mut mappings: Vec<_> = args.iter().collect();
        if let Some(kwargs) = kwargs {
            mappings.push(kwargs.clone().into_any());
        }
        for mapping in mappings {
            let mapping = if mapping.hasattr("dicts")? {
                mapping.getattr("dicts")?.get_item(-1)?
            } else {
                mapping
            };
            for item in mapping.call_method0("items")?.try_iter()? {
                let (key, value): (String, Bound<'py, PyAny>) = item?.extract()?;
                self.__setitem__(py, key, value)?;
            }
        }
        Ok(scope)
    }

    /// Django's `Context.pop()`. A tag can only pop what it pushed.
    fn pop(&self, py: Python<'_>) -> PyResult<()> {
        let mut guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        if !guard.callback_scope_open() {
            drop(guard);
            let error = py
                .import("django.template.context")?
                .getattr("ContextPopException")?;
            return Err(PyErr::from_value(error.call0()?));
        }
        guard.pop_variables();
        drop(guard);
        Ok(())
    }

    /// Like Django's `Context.setdefault()`.
    fn setdefault<'py>(
        &self,
        py: Python<'py>,
        key: String,
        default: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        match self.__getitem__(py, key.clone()) {
            Ok(value) => Ok(value),
            Err(error) if error.is_instance_of::<PyKeyError>(py) => {
                let default = default.unwrap_or_else(|| py.None().into_bound(py));
                self.__setitem__(py, key, default.clone())?;
                Ok(default)
            }
            Err(error) => Err(error),
        }
    }

    /// Like Django's `Context.set_upward()`.
    fn set_upward<'py>(
        &self,
        py: Python<'py>,
        key: String,
        value: Bound<'py, PyAny>,
    ) -> PyResult<()> {
        let mut guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        // As in `__setitem__`, a replaced value is dropped after the lock.
        let replaced = guard.get(py, &key);
        let result = guard.set_upward(key, value);
        drop(guard);
        drop(replaced);
        result
    }

    /// Django's `Context.render_context`: state a tag keeps for the render.
    #[getter]
    fn render_context<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        if let Some(render_context) = &guard.render_context {
            return Ok(render_context.bind(py).clone());
        }
        let render_context = py
            .import("django.template.context")?
            .getattr("RenderContext")?
            .call0()?;
        guard.render_context = Some(render_context.clone().unbind());
        drop(guard);
        Ok(render_context)
    }

    /// Django's `Context.new()`: a context with only `values`, and this
    /// one's request and settings.
    #[pyo3(name = "new", signature = (values=None))]
    fn new_context(&self, py: Python<'_>, values: Option<Bound<'_, PyAny>>) -> PyResult<Self> {
        let mut bindings: HashMap<String, Py<PyAny>> = HashMap::from([
            (
                "True".to_string(),
                PyBool::new(py, true).to_owned().into_any().unbind(),
            ),
            (
                "False".to_string(),
                PyBool::new(py, false).to_owned().into_any().unbind(),
            ),
            ("None".to_string(), py.None()),
        ]);
        if let Some(values) = values.filter(|values| !values.is_none()) {
            let values = if values.hasattr("flatten")? {
                values.call_method0("flatten")?
            } else {
                values
            };
            for item in values.call_method0("items")?.try_iter()? {
                let (key, value): (String, Py<PyAny>) = item?.extract()?;
                bindings.insert(key, value);
            }
        }
        let guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        let request = guard.request.as_ref().map(|request| request.clone_ref(py));
        let mut context = Context::new(bindings, request, guard.autoescape);
        context.use_l10n = guard.use_l10n;
        context.use_tz = guard.use_tz;
        context.format_values = guard.format_values;
        context.render_stack = guard.render_stack;
        context.number_format = Arc::clone(&guard.number_format);
        drop(guard);
        Ok(Self::new(context))
    }

    /// Django's copy is shallow: it shares the layers, so a value set
    /// through the copy shows in the original.
    fn __copy__(&self) -> Self {
        self.clone()
    }

    /// Like Django's `Context.flatten()`.
    fn flatten<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        let values = guard.as_dict(py)?;
        if let Some(forloop) = guard.resolve_root(py, "forloop")?
            && !values.contains("forloop")?
        {
            values.set_item("forloop", forloop)?;
        }
        drop(guard);
        Ok(values)
    }
}

impl PyContext {
    /// Open a scope on the render's context and remember where it began.
    fn open_scope(&self, py: Python<'_>) -> PyContextScope {
        let mut guard = self
            .context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned");
        let depth = guard.scope_depth();
        guard.push_scope(&[]);
        drop(guard);
        PyContextScope {
            context: self.context.clone(),
            depth,
        }
    }
}

/// What `push()` and `update()` return: leaving its `with` block closes the
/// scope, as Django's `ContextDict` does.
#[pyclass]
pub struct PyContextScope {
    context: Gc<Mutex<Context>>,
    depth: usize,
}

#[pymethods]
impl PyContextScope {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.context.trace(&visit)
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[pyo3(signature = (*_args))]
    fn __exit__(&self, py: Python<'_>, _args: &Bound<'_, PyTuple>) {
        self.context
            .lock_py_attached(py)
            .expect("Mutex should not be poisoned")
            .pop_scope_above(self.depth);
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
        false => {
            let text = value.str()?;
            // `str()` nearly always gives an exact `str`, which has no
            // `__html__` to look for.
            if text.is_exact_instance_of::<PyString>() {
                return Ok(ContentString::HtmlUnsafe(text.extract::<String>()?.into()));
            }
            text.into_any()
        }
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
pub(crate) fn append_escaped(text: &str, output: &mut String) {
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

/// False only for an exact `str` or `SafeString` without a numeric character,
/// which `int()` would reject.
fn has_digit(value: &Bound<'_, PyAny>) -> bool {
    let plain = value.is_exact_instance_of::<PyString>()
        || crate::render::filters::is_exact_safe_string(value).unwrap_or(false);
    if !plain {
        return true;
    }
    value
        .cast::<PyString>()
        .ok()
        .and_then(|text| {
            text.to_cow()
                .ok()
                .map(|text| text.chars().any(char::is_numeric))
        })
        .unwrap_or(true)
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

/// Prints an integer Rusty computed itself, localized as Django's
/// `localize()` prints an `int`.
pub fn render_int(number: &BigInt, context: &Context) -> PyResult<String> {
    if let Some(number) = localize_int(number, context)? {
        return Ok(number);
    }
    Python::attach(|py| {
        Content::Py(number.into_pyobject(py)?.into_any())
            .render(context)
            .map(Cow::into_owned)
    })
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
                    && let Some(number) = localize_number(&content, context.use_l10n, context)?
                {
                    if context.autoescape {
                        append_escaped(&number, output);
                    } else {
                        output.push_str(&number);
                    }
                    return Ok(());
                }
                // Inside `{% localize %}` the `use_l10n` override goes to Django.
                if context.format_values
                    && context.use_l10n.is_none()
                    && let Some(formatted) = localize_datetime(
                        &content,
                        context.use_tz,
                        &mut context.date_cache.borrow_mut(),
                    )?
                {
                    if context.autoescape {
                        append_escaped(&formatted, output);
                    } else {
                        output.push_str(&formatted);
                    }
                    return Ok(());
                }
                // Django's `render_value_in_context` changes only numbers and
                // dates; other values print natively even in a live context.
                let string = content.is_instance_of::<PyString>();
                if context.format_values
                    && ((string && context.formatting_context.is_some())
                        || (!string && is_localizable(&content)?))
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
                    let text = content.str()?.into_any();
                    // `str()` nearly always gives an exact `str`, which has
                    // no `__html__` to look for.
                    if text.is_exact_instance_of::<PyString>() {
                        return append_python_string(&text, true, output);
                    }
                    text
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
                if let Some(number) = localize_number(&content, context.use_l10n, context)? {
                    return Ok(Cow::Owned(if context.autoescape {
                        encode_quoted_attribute(&number).into_owned()
                    } else {
                        number
                    }));
                }
                if context.use_l10n.is_none()
                    && let Some(formatted) = localize_datetime(
                        &content,
                        context.use_tz,
                        &mut context.date_cache.borrow_mut(),
                    )?
                {
                    return Ok(Cow::Owned(if context.autoescape {
                        encode_quoted_attribute(&formatted).into_owned()
                    } else {
                        formatted
                    }));
                }
                if is_localizable(&content)? {
                    let py = content.py();
                    let rendered = RENDER_VALUE
                        .import(py, "django.template.base", "render_value_in_context")?
                        .call1((content, context.value_formatting_context(py)?))?;
                    return Ok(Cow::Owned(rendered.extract()?));
                }
                resolve_python(content, context)?
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
            Self::Float(content) => return Ok(python_float_str(content).into()),
            Self::Int(content) => return render_int(&content, context).map(Cow::Owned),
            Self::Bool(true) => return Ok(Cow::Borrowed("True")),
            Self::Bool(false) => return Ok(Cow::Borrowed("False")),
        };
        Ok(render_value_in_context(output, context))
    }

    pub fn resolve_string(self, context: &Context) -> PyResult<ContentString<'t>> {
        Ok(match self {
            Self::String(content) => content,
            Self::Float(content) => ContentString::String(python_float_str(content).into()),
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
            Self::String(left) => {
                let text = left.as_raw();
                match text.parse::<BigInt>() {
                    Ok(number) => Some(number),
                    // Python's `int()` also takes spaces, underscores and
                    // other Unicode digits.
                    Err(_) if text.chars().any(char::is_numeric) => Python::attach(|py| {
                        PyType::new::<PyInt>(py)
                            .call1((text.as_ref(),))
                            .ok()?
                            .extract::<BigInt>()
                            .ok()
                    }),
                    Err(_) => None,
                }
            }
            Self::Float(left) => left.trunc().to_bigint(),
            Self::Py(left) => match left.extract::<BigInt>() {
                Ok(left) => Some(left),
                // `int()` needs a digit, and neither `str` nor `SafeString`
                // overrides `__int__`, so skip a call that can only raise.
                Err(_) if !has_digit(left) => None,
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
                // Django calls `int(arg)`: a float or `Decimal` truncates, a
                // string is parsed, and anything else raises Python's error.
                Err(_) => {
                    let number = PyType::new::<PyInt>(obj.py()).call1((&obj,))?;
                    resolve_bigint(number.extract()?, argument_at)
                }
            },
            Self::Bool(true) => Ok(1),
            Self::Bool(false) => Ok(0),
        }
    }
    /// A float infinity, native or from the context. Python's `int()`
    /// raises `OverflowError` for one.
    pub fn is_infinite(&self) -> bool {
        match self {
            Self::Float(number) => number.is_infinite(),
            Self::Py(object) if object.is_exact_instance_of::<pyo3::types::PyFloat>() => {
                object.extract::<f64>().is_ok_and(f64::is_infinite)
            }
            _ => false,
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
                    // `mark_safe` of a plain `str` is `SafeString(str)`;
                    // calling the class skips a Python function call.
                    let safe_string = SAFE_STRING
                        .import(py, "django.utils.safestring", "SafeString")
                        .expect("Should be able to import `django.utils.safestring.SafeString`");
                    safe_string
                        .call1((string,))
                        .expect("`SafeString` should not raise if given a string")
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use pyo3::prelude::*;
    use pyo3::types::PyDict;

    use super::Context;

    fn context_with_source(py: Python<'_>) -> (Context, Bound<'_, PyDict>) {
        let source = PyDict::new(py);
        source.set_item("name", "caller").unwrap();
        source.set_item("True", "shadowed").unwrap();
        let builtins = HashMap::from([
            (
                "True".to_string(),
                true.into_pyobject(py)
                    .unwrap()
                    .to_owned()
                    .into_any()
                    .unbind(),
            ),
            (
                "other".to_string(),
                1_i64.into_pyobject(py).unwrap().into_any().unbind(),
            ),
        ]);
        (
            Context::new(builtins, None, false).with_source(&source),
            source,
        )
    }

    #[test]
    fn test_source_values_take_precedence_over_initial_bindings() {
        Python::initialize();
        Python::attach(|py| {
            let (context, _) = context_with_source(py);
            let value = context.get(py, "True").unwrap();
            assert_eq!(value.extract::<String>().unwrap(), "shadowed");
            assert_eq!(
                context.get(py, "other").unwrap().extract::<i64>().unwrap(),
                1
            );
            assert!(context.get(py, "missing").is_none());
        });
    }

    #[test]
    fn test_cleared_source_value_is_hidden() {
        Python::initialize();
        Python::attach(|py| {
            let (mut context, source) = context_with_source(py);
            context.clear_variable("name");
            assert!(context.get(py, "name").is_none());
            let dict = context.as_dict(py).unwrap();
            assert!(!dict.contains("name").unwrap());
            assert!(dict.contains("True").unwrap());
            assert!(!context.display(py).contains("caller"));
            // The caller's dict is never changed.
            assert!(source.contains("name").unwrap());
        });
    }

    #[test]
    fn test_writes_stack_on_source_values() {
        Python::initialize();
        Python::attach(|py| {
            let (mut context, _) = context_with_source(py);
            context.push_variable(
                "name".to_string(),
                "loop".into_pyobject(py).unwrap().into_any(),
                0,
            );
            assert_eq!(
                context
                    .get(py, "name")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "loop"
            );
            context.pop_variables();
            assert_eq!(
                context
                    .get(py, "name")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "caller"
            );
        });
    }
}
