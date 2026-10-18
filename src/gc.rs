//! Shared native objects whose Python references participate in cyclic GC.
//!
//! An `Arc` containing Python references hides both its ownership and any cycles
//! from Python. Each `Gc` clone instead owns a Python reference to one shared,
//! immutable container. Traversal visits that container, so shared syntax trees
//! are traced once, rather than reporting their Python references repeatedly.

use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use pyo3::prelude::*;
use pyo3::sync::MutexExt;
use pyo3::{PyTraverseError, PyVisit};

pub trait Trace {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError>;

    fn clear(&self) {}

    // Only state which can move back out of its container needs a native
    // handle count. Immutable syntax trees use Python ownership alone.
    fn count_native_owners(&self) -> bool {
        false
    }
}

trait Value: Trace + Any + Send + Sync + Debug {
    fn as_any(&self) -> &dyn Any;
}

impl<T: Trace + Any + Send + Sync + Debug> Value for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[pyclass(frozen)]
struct Shared {
    value: Box<dyn Value>,
    // For movable state, count native handles separately from Python's GC references.
    // This also works on free-threaded Python, where Py_REFCNT cannot prove
    // that a value has a single owner.
    native_owners: AtomicUsize,
}

#[pymethods]
impl Shared {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.value.trace(&visit)
    }

    fn __clear__(&self) {
        self.value.clear();
    }
}

pub struct Gc<T> {
    shared: Py<Shared>,
    marker: PhantomData<T>,
}

impl<T: Trace + Any + Send + Sync + Debug> Gc<T> {
    pub fn new(value: T) -> Self {
        let native_owners = usize::from(value.count_native_owners());
        Python::attach(|py| Self {
            shared: Py::new(
                py,
                Shared {
                    value: Box::new(value),
                    native_owners: AtomicUsize::new(native_owners),
                },
            )
            .expect("allocate shared native object"),
            marker: PhantomData,
        })
    }

    pub fn ptr_eq(left: &Self, right: &Self) -> bool {
        left.shared.as_ptr() == right.shared.as_ptr()
    }

    pub fn as_ptr(value: &Self) -> *mut pyo3::ffi::PyObject {
        value.shared.as_ptr()
    }
}

impl<T> Clone for Gc<T> {
    fn clone(&self) -> Self {
        let owners = &self.shared.get().native_owners;
        if owners.load(Ordering::Relaxed) != 0 {
            owners.fetch_add(1, Ordering::Relaxed);
        }
        Python::attach(|py| Self {
            shared: self.shared.clone_ref(py),
            marker: PhantomData,
        })
    }
}

impl<T> Drop for Gc<T> {
    fn drop(&mut self) {
        let owners = &self.shared.get().native_owners;
        if owners.load(Ordering::Relaxed) != 0 {
            owners.fetch_sub(1, Ordering::Release);
        }
    }
}

impl<T: Default + Debug + Send + 'static> Gc<Mutex<T>>
where
    Mutex<T>: Trace,
{
    /// Return exclusive callback state without copying it. Retained Python
    /// wrappers and block callbacks own native handles too, so their snapshots
    /// still take the cloning path. Consuming this handle prevents new aliases
    /// when its native owner count is one. Untracked state always clones.
    pub fn take_or_clone(self, py: Python<'_>, clone: impl FnOnce(&T) -> T) -> T {
        let unique = self.shared.get().native_owners.load(Ordering::Acquire) == 1;
        let mut value = self
            .lock_py_attached(py)
            .expect("shared value lock is not poisoned");
        if unique {
            std::mem::take(&mut *value)
        } else {
            clone(&value)
        }
    }
}

impl<T: Trace + Any + Send + Sync + Debug> From<T> for Gc<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: 'static> Deref for Gc<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.shared
            .get()
            .value
            .as_any()
            .downcast_ref()
            .expect("shared native object has its declared type")
    }
}

impl<T: 'static> AsRef<T> for Gc<T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: Debug + 'static> Debug for Gc<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(formatter)
    }
}

impl<T: PartialEq + 'static> PartialEq for Gc<T> {
    fn eq(&self, other: &Self) -> bool {
        self.deref() == other.deref()
    }
}

impl<T> Trace for Gc<T> {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.shared)
    }
}

impl<T> Trace for Py<T> {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(self)
    }
}

impl<T: Trace> Trace for Vec<T> {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.iter().try_for_each(|value| value.trace(visit))
    }
}

impl<T: Trace> Trace for Option<T> {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.as_ref().map_or(Ok(()), |value| value.trace(visit))
    }
}

impl<T: Trace> Trace for Box<T> {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.as_ref().trace(visit)
    }
}

impl<K, T: Trace> Trace for HashMap<K, T> {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.values().try_for_each(|value| value.trace(visit))
    }
}

impl<T: Trace> Trace for OnceLock<T> {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.get().map_or(Ok(()), |value| value.trace(visit))
    }
}

/// A fallible Python initializer with a nonblocking, Python-free GC read path.
#[derive(Debug)]
pub(crate) struct GcOnceLock<T> {
    value: OnceLock<T>,
    initializing: Mutex<()>,
}

impl<T> GcOnceLock<T> {
    pub fn get(&self) -> Option<&T> {
        self.value.get()
    }

    pub fn new() -> Self {
        Self {
            value: OnceLock::new(),
            initializing: Mutex::new(()),
        }
    }

    pub fn get_or_try_init<E>(
        &self,
        py: Python<'_>,
        init: impl FnOnce() -> Result<T, E>,
    ) -> Result<&T, E> {
        if let Some(value) = self.value.get() {
            return Ok(value);
        }
        let _guard = self
            .initializing
            .lock_py_attached(py)
            .expect("initializer lock is not poisoned");
        if let Some(value) = self.value.get() {
            return Ok(value);
        }
        let _ = self.value.set(init()?);
        Ok(self.value.get().expect("initialized native object"))
    }
}

impl<T: Trace> Trace for GcOnceLock<T> {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.value.trace(visit)
    }
}

impl<A: Trace, T: Trace> Trace for (A, T) {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.0.trace(visit)?;
        self.1.trace(visit)
    }
}

impl Trace for String {
    fn trace(&self, _visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        Ok(())
    }
}

impl Trace for usize {
    fn trace(&self, _visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        Ok(())
    }
}

// Only fields which can own Python references need tracing. Native source
// spans, names, numbers, and literal strings have no GC edges.
macro_rules! trace_fields {
    ($($ty:ty => $($field:tt),+;)+) => {$ (
        impl Trace for $ty {
            fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
                $(self.$field.trace(visit)?;)+
                Ok(())
            }
        }
    )+};
}

use crate::filters::FilterType;
use crate::parse::{self, IfCondition, StockNode, Tag, TagElement, TokenTree};

trace_fields! {
    parse::Filter => left, filter;
    parse::InclusionTag => call, filename, engine;
    parse::Include => template_name, engine, kwargs;
    parse::SimpleTag => func, args, kwargs;
    parse::SimpleBlockTag => func, nodes, args, kwargs;
    parse::PythonNode => 0;
    parse::Regroup => target, expression;
    parse::PartialDefinition => nodes;
    parse::Url => view_name, args, kwargs;
    parse::For => iterable, body, empty;
    parse::ForIterable => iterable;
    parse::CycleValue => value;
    parse::SimpleCycle => values;
    parse::NamedCycle => cycle;
    parse::SilentNamedCycle => cycle;
}

impl Trace for FilterType {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        match self {
            Self::Django(filter) => filter.filter.trace(visit),
            Self::External(filter) => filter.filter.trace(visit),
            Self::Add(_)
            | Self::AddSlashes(_)
            | Self::Capfirst(_)
            | Self::Center(_)
            | Self::Cut(_)
            | Self::Default(_)
            | Self::DefaultIfNone(_)
            | Self::DivisibleBy(_)
            | Self::Date(_)
            | Self::Escape(_)
            | Self::Escapejs(_)
            | Self::ForceEscape(_)
            | Self::Last(_)
            | Self::Lower(_)
            | Self::Length(_)
            | Self::Safe(_)
            | Self::Slugify(_)
            | Self::Title(_)
            | Self::Upper(_)
            | Self::Wordcount(_)
            | Self::Wordwrap(_)
            | Self::Yesno(_) => Ok(()),
        }
    }
}

impl Trace for TagElement {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        match self {
            Self::Filter(filter) => filter.trace(visit),
            Self::Int(_)
            | Self::Float(_)
            | Self::Text(_)
            | Self::TranslatedText(_)
            | Self::Variable(_)
            | Self::ForVariable(_) => Ok(()),
        }
    }
}

impl Trace for parse::IncludeTemplateName {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        match self {
            Self::Variable(value) => value.trace(visit),
            Self::Text(_) | Self::Relative(_) => Ok(()),
        }
    }
}

impl Trace for IfCondition {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        match self {
            Self::Variable(value) => value.trace(visit),
            Self::Not(value) => value.trace(visit),
            Self::And(pair)
            | Self::Or(pair)
            | Self::Equal(pair)
            | Self::NotEqual(pair)
            | Self::LessThan(pair)
            | Self::GreaterThan(pair)
            | Self::LessThanEqual(pair)
            | Self::GreaterThanEqual(pair)
            | Self::In(pair)
            | Self::NotIn(pair)
            | Self::Is(pair)
            | Self::IsNot(pair) => pair.trace(visit),
        }
    }
}

impl Trace for parse::Cycle {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        match self {
            Self::Simple(value) => value.trace(visit),
            Self::Named(value) => value.trace(visit),
            Self::SilentNamed(value) => value.trace(visit),
        }
    }
}

impl Trace for TokenTree {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        match self {
            Self::Tag(tag) => tag.trace(visit),
            Self::Filter(filter) => filter.trace(visit),
            Self::Text(_)
            | Self::LiteralText(_)
            | Self::TranslatedText(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::Variable(_)
            | Self::ForVariable(_) => Ok(()),
        }
    }
}

impl Trace for StockNode {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        match self {
            Self::Static { path, .. } => path.trace(visit)?,
            Self::Prefix { .. } => {}
            Self::Flag { nodes, .. } => nodes.trace(visit)?,
            Self::Override { value, nodes, .. } => {
                value.trace(visit)?;
                nodes.trace(visit)?;
            }
            Self::Assign { value, .. } => value.trace(visit)?,
            Self::Translate {
                root,
                filters,
                message_context,
                ..
            } => {
                root.trace(visit)?;
                filters.trace(visit)?;
                message_context.trace(visit)?;
            }
            Self::BlockTranslate {
                bindings,
                counter,
                message_context,
                ..
            } => {
                bindings.trace(visit)?;
                counter.trace(visit)?;
                message_context.trace(visit)?;
            }
            Self::Cache {
                timeout,
                vary,
                cache,
                nodes,
                ..
            } => {
                timeout.trace(visit)?;
                vary.trace(visit)?;
                cache.trace(visit)?;
                nodes.trace(visit)?;
            }
        }
        Ok(())
    }
}

impl Trace for Tag {
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        match self {
            Self::Stock { node, .. } => node.trace(visit)?,
            Self::PartialDef { definition, .. } => definition.trace(visit)?,
            Self::PythonNode(node) => node.trace(visit)?,
            Self::Block { nodes, .. } => nodes.trace(visit)?,
            Self::Autoescape { nodes, .. } | Self::Spaceless(nodes) | Self::Verbatim(nodes) => {
                nodes.trace(visit)?
            }
            Self::Extends { parent, nodes, .. } => {
                parent.trace(visit)?;
                nodes.trace(visit)?;
            }
            Self::If {
                condition,
                truthy,
                falsey,
            } => {
                condition.trace(visit)?;
                truthy.trace(visit)?;
                falsey.trace(visit)?;
            }
            Self::IfChanged {
                values,
                truthy,
                falsey,
                ..
            } => {
                values.trace(visit)?;
                truthy.trace(visit)?;
                falsey.trace(visit)?;
            }
            Self::Regroup(value) => value.trace(visit)?,
            Self::For(value) => value.trace(visit)?,
            Self::Include(value) => value.trace(visit)?,
            Self::With { assignments, nodes } => {
                assignments.trace(visit)?;
                nodes.trace(visit)?;
            }
            Self::FilterBlock { filters, nodes } => {
                filters.trace(visit)?;
                nodes.trace(visit)?;
            }
            Self::QueryString(value) | Self::SimpleTag(value) => value.trace(visit)?,
            Self::InclusionTag(value) => value.trace(visit)?,
            Self::SimpleBlockTag(value) => value.trace(visit)?,
            Self::Url(value) => value.trace(visit)?,
            Self::Lorem(value) => value.count.trace(visit)?,
            Self::Now(value) => value.expression.trace(visit)?,
            Self::FirstOf(value) => value.vars.trace(visit)?,
            Self::WidthRatio { args, .. } => args.trace(visit)?,
            Self::Cycle(value) => value.trace(visit)?,
            Self::Partial { .. }
            | Self::Load
            | Self::ResetCycle(_)
            | Self::Debug
            | Self::CsrfToken(_)
            | Self::Comment(_)
            | Self::TemplateTag(_) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A native buffer lets these tests distinguish moving from copying without
    // relying on timings or CPython reference-count implementation details.
    impl Trace for Mutex<Vec<String>> {
        fn count_native_owners(&self) -> bool {
            true
        }

        fn trace(&self, _visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
            Ok(())
        }
    }

    impl Trace for Mutex<Vec<usize>> {
        fn trace(&self, _visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
            Ok(())
        }
    }

    #[test]
    fn untracked_state_preserves_other_owners() {
        Python::initialize();
        Python::attach(|py| {
            let shared = Gc::new(Mutex::new(vec![1_usize]));
            let retained = shared.clone();
            let returned = shared.take_or_clone(py, Clone::clone);
            assert_eq!(returned, [1]);
            assert_eq!(*retained.lock().expect("test lock is not poisoned"), [1]);
        });
    }

    #[test]
    fn unique_state_reuses_its_buffer() {
        Python::initialize();
        Python::attach(|py| {
            let buffer = vec!["value".to_string()];
            let pointer = buffer.as_ptr();
            let shared = Gc::new(Mutex::new(buffer));
            let returned = shared.take_or_clone(py, Clone::clone);
            assert_eq!(returned, ["value"]);
            assert_eq!(returned.as_ptr(), pointer);
        });
    }

    #[test]
    fn retained_state_keeps_its_buffer() {
        Python::initialize();
        Python::attach(|py| {
            let shared = Gc::new(Mutex::new(vec!["value".to_string()]));
            let retained = shared.clone();
            let returned = shared.take_or_clone(py, Clone::clone);
            let retained = retained.lock().expect("test lock is not poisoned");
            assert_eq!(*retained, returned);
            assert_ne!(retained.as_ptr(), returned.as_ptr());
        });
    }

    #[test]
    fn released_alias_allows_moving_state() {
        Python::initialize();
        Python::attach(|py| {
            let buffer = vec!["value".to_string()];
            let pointer = buffer.as_ptr();
            let shared = Gc::new(Mutex::new(buffer));
            drop(shared.clone());
            let returned = shared.take_or_clone(py, Clone::clone);
            assert_eq!(returned.as_ptr(), pointer);
        });
    }
}
