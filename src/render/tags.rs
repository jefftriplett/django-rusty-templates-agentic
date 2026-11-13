use crate::gc::Gc;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};

use either::Either;
use num_bigint::BigInt;
use num_traits::cast::{FromPrimitive, ToPrimitive};
use pyo3::basic::CompareOp;
use pyo3::exceptions::{
    PyAttributeError, PyException, PyKeyError, PyOverflowError, PyTypeError, PyValueError,
    PyZeroDivisionError,
};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::MutexExt;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyNone, PyString, PyTuple};

use crate::parse::Now;
use crate::render::dateformat::Formatter;
use crate::render::filters::{DATE_FILTER, ResolveFilter, format_localtime};
use crate::render::lorem::{COMMON_WORDS, paragraphs, words};
use dtl_lexer::tag::lorem::LoremMethod;
use dtl_lexer::types::{At, TemplateString};

use super::types::{
    AsBorrowedContent, Content, ContentString, Context, IncludeTemplateKey, PyContext,
};
use super::{Evaluate, Render, RenderResult, Resolve, ResolveFailures, ResolveResult};
use crate::error::{AnnotatePyErr, PyRenderError, RenderError};
use crate::parse::{
    CsrfToken, Cycle, CycleValue, FirstOf, For, IfCondition, Include, IncludeTemplateName,
    InclusionTag, Lorem, NamedCycle, SilentNamedCycle, SimpleBlockTag, SimpleCycle, SimpleTag, Tag,
    TagElement, Url,
};
use crate::path::construct_relative_path;
use crate::template::django_rusty_templates::{
    NoReverseMatch, Template, TemplateDoesNotExist, TemplateSyntaxError, VariableDoesNotExist,
};
use crate::utils::PyResultMethods;

static RENDER_FIRSTOF_VALUE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static FIRSTOF_SAFE_STRING: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static QUERY_DICT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static QUERY_SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static GROUPED_RESULT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DJANGO_CONTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static MARK_SAFE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DEBUG_NODE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static MARK_BLOCK_SAFE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static CONDITIONAL_ESCAPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static INCLUSION_VALUES: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

static BUILTINS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

static PROMISE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static REVERSE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static WARNINGS_WARN: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DJANGO_SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

fn current_app(py: Python, request: Option<&Py<PyAny>>) -> PyResult<Py<PyAny>> {
    let Some(request) = request else {
        return Ok(py.None());
    };
    if let Ok(current_app) = request
        .getattr(py, "current_app")
        .ok_or_isinstance_of::<PyAttributeError>(py)?
    {
        return Ok(current_app);
    }
    match request
        .getattr(py, "resolver_match")
        .ok_or_isinstance_of::<PyAttributeError>(py)?
    {
        Ok(resolver_match) if !resolver_match.is_none(py) => {
            resolver_match.getattr(py, "namespace")
        }
        _ => Ok(py.None()),
    }
}

impl Resolve for Url {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
        failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        let view_name = match self.view_name.resolve(py, template, context, failures)? {
            Some(view_name) => view_name,
            None => "".as_content(),
        };
        let reverse = REVERSE.import(py, "django_rusty_templates.diagnostics", "render_url")?;

        // Django resolves every argument, then `reverse()` refuses the mix.
        if !self.args.is_empty() && !self.kwargs.is_empty() {
            for argument in self
                .args
                .iter()
                .chain(self.kwargs.iter().map(|(_, value)| value))
            {
                argument.resolve(py, template, context, failures)?;
            }
            return Err(PyValueError::new_err(
                "Don't mix *args and **kwargs in call to reverse()!",
            )
            .into());
        }

        let current_app = current_app(py, context.request.as_ref())?;
        let url = if self.kwargs.is_empty() {
            let py_args = PyList::empty(py);
            for arg in &self.args {
                py_args.append(
                    arg.resolve(py, template, context, failures)?
                        .unwrap_or_else(|| "".as_content()),
                )?;
            }
            reverse.call1((
                view_name,
                py.None(),
                py_args.to_tuple(),
                py.None(),
                current_app,
            ))
        } else {
            let kwargs = PyDict::new(py);
            for (key, value) in &self.kwargs {
                kwargs.set_item(key, value.resolve(py, template, context, failures)?)?;
            }
            reverse.call1((view_name, py.None(), py.None(), kwargs, current_app))
        };
        let url = match url {
            Ok(url) => Ok(url),
            Err(error) => Err(error.annotate(py, self.at, "here", template)),
        };
        match &self.asvar {
            None => Ok(Some(Content::Py(url?))),
            Some(variable) => {
                let value = match url.ok_or_isinstance_of::<NoReverseMatch>(py)? {
                    Ok(url) => url,
                    Err(_) => PyString::new(py, "").into_any(),
                };
                context.insert(variable.clone(), value)?;
                Ok(None)
            }
        }
    }
}

/// Django's `{% if %}` comparisons, which are Python's. A comparison that
/// raises is false.
trait PyCmp<T> {
    fn compare(&self, other: &T, op: CompareOp) -> bool;
}

/// The operator Python tries on the right operand: `a < b` falls back to
/// `b.__gt__(a)`.
const fn reflected(op: CompareOp) -> CompareOp {
    match op {
        CompareOp::Eq => CompareOp::Eq,
        CompareOp::Ne => CompareOp::Ne,
        CompareOp::Lt => CompareOp::Gt,
        CompareOp::Gt => CompareOp::Lt,
        CompareOp::Le => CompareOp::Ge,
        CompareOp::Ge => CompareOp::Le,
    }
}

/// Python's `!=` calls `__ne__`, which a class can define apart from
/// `__eq__`, so every operator goes to Python as itself.
fn python_compare(obj: &Bound<'_, PyAny>, other: &Content<'_, '_>, op: CompareOp) -> bool {
    match other {
        Content::Py(other) => obj.rich_compare(other, op),
        Content::Float(other) => obj.rich_compare(other, op),
        Content::Int(other) => obj.rich_compare(other, op),
        Content::Bool(other) => obj.rich_compare(other, op),
        Content::String(other) => obj.rich_compare(other.as_raw(), op),
    }
    .and_then(|result| result.is_truthy())
    .unwrap_or(false)
}

/// How two native values order, as Python orders them, or `None` when
/// Python can't order them (different types, or a NaN).
fn native_ordering(left: &Content<'_, '_>, right: &Content<'_, '_>) -> Option<Ordering> {
    match (left, right) {
        (Content::Float(left), Content::Float(right)) => left.partial_cmp(right),
        (Content::Int(left), Content::Int(right)) => Some(left.cmp(right)),
        (Content::Bool(left), Content::Bool(right)) => Some(left.cmp(right)),
        (Content::Int(left), Content::Bool(right)) => {
            Some(left.cmp(&BigInt::from(u8::from(*right))))
        }
        (Content::Bool(left), Content::Int(right)) => {
            Some(BigInt::from(u8::from(*left)).cmp(right))
        }
        (Content::Float(left), Content::Bool(right)) => left.partial_cmp(&f64::from(*right)),
        (Content::Bool(left), Content::Float(right)) => f64::from(*left).partial_cmp(right),
        (Content::Float(left), Content::Int(right)) => float_int_ordering(*left, right),
        (Content::Int(left), Content::Float(right)) => {
            float_int_ordering(*right, left).map(Ordering::reverse)
        }
        (Content::String(left), Content::String(right)) => Some(left.as_raw().cmp(right.as_raw())),
        _ => None,
    }
}

/// An int too large for a float lies beyond every finite float, but not
/// beyond the infinity of the same sign.
fn float_int_ordering(float: f64, int: &BigInt) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    match int.to_f64().expect("BigInt to f64 is always possible") {
        f64::INFINITY if float == f64::INFINITY => Some(Ordering::Greater),
        f64::INFINITY => Some(Ordering::Less),
        f64::NEG_INFINITY if float == f64::NEG_INFINITY => Some(Ordering::Less),
        f64::NEG_INFINITY => Some(Ordering::Greater),
        int => float.partial_cmp(&int),
    }
}

fn holds(ordering: Option<Ordering>, op: CompareOp) -> bool {
    match op {
        CompareOp::Eq => ordering.is_some_and(Ordering::is_eq),
        CompareOp::Ne => !ordering.is_some_and(Ordering::is_eq),
        CompareOp::Lt => ordering.is_some_and(Ordering::is_lt),
        CompareOp::Le => ordering.is_some_and(Ordering::is_le),
        CompareOp::Gt => ordering.is_some_and(Ordering::is_gt),
        CompareOp::Ge => ordering.is_some_and(Ordering::is_ge),
    }
}

impl PyCmp<Content<'_, '_>> for Content<'_, '_> {
    fn compare(&self, other: &Content<'_, '_>, op: CompareOp) -> bool {
        match (self, other) {
            (Self::Py(obj), other) => python_compare(obj, other, op),
            // The Python object is on the right, so its reflected method
            // runs, as it would after the native type returns
            // `NotImplemented`.
            (_, Content::Py(other)) => python_compare(other, self, reflected(op)),
            _ => holds(native_ordering(self, other), op),
        }
    }
}

impl PyCmp<Self> for Option<Content<'_, '_>> {
    fn compare(&self, other: &Self, op: CompareOp) -> bool {
        match (self, other) {
            (Some(obj), Some(other)) => obj.compare(other, op),
            (None, None) => matches!(op, CompareOp::Eq),
            (Some(obj), None) | (None, Some(obj)) => match (op, obj) {
                (CompareOp::Eq | CompareOp::Ne, Content::Py(obj)) => obj
                    .rich_compare(PyNone::get(obj.py()), op)
                    .and_then(|result| result.is_truthy())
                    .unwrap_or(false),
                (CompareOp::Ne, _) => true,
                _ => false,
            },
        }
    }
}

trait Contains<T> {
    fn contains(&self, other: T) -> Option<bool>;
}

impl Contains<Option<Content<'_, '_>>> for Content<'_, '_> {
    fn contains(&self, other: Option<Content<'_, '_>>) -> Option<bool> {
        match other {
            None => match self {
                Self::Py(obj) => obj.contains(PyNone::get(obj.py())).ok(),
                _ => None,
            },
            Some(Content::Py(other)) => {
                let obj = self.to_py(other.py());
                obj.contains(other).ok()
            }
            Some(Content::String(other)) => match self {
                Self::String(obj) => Some(obj.as_raw().contains(other.as_raw().as_ref())),
                Self::Int(_) | Self::Float(_) | Self::Bool(_) => None,
                Self::Py(obj) => obj.contains(other).ok(),
            },
            Some(Content::Int(n)) => match self {
                Self::Py(obj) => obj.contains(n).ok(),
                _ => None,
            },
            Some(Content::Float(f)) => match self {
                Self::Py(obj) => obj.contains(f).ok(),
                _ => None,
            },
            Some(Content::Bool(b)) => match self {
                Self::Py(obj) => obj.contains(b).ok(),
                _ => None,
            },
        }
    }
}

/// Whether Django's `except Exception` around each operator catches `error`.
/// `KeyboardInterrupt` and other `BaseException`s escape.
fn operator_catches(py: Python<'_>, error: &PyRenderError) -> bool {
    match error {
        PyRenderError::PyErr(error) => error.is_instance_of::<PyException>(py),
        PyRenderError::RenderError(_) | PyRenderError::InvalidTagResult(_) => true,
    }
}

/// Python's `bool(value)`; a missing variable is `None`.
fn truth(value: Option<&Content<'_, '_>>) -> PyResult<bool> {
    Ok(match value {
        None => false,
        Some(Content::Py(object)) => object.is_truthy()?,
        Some(Content::String(text)) => !text.as_raw().is_empty(),
        Some(Content::Float(number)) => *number != 0.0,
        Some(Content::Int(number)) => *number != BigInt::ZERO,
        Some(Content::Bool(value)) => *value,
    })
}

type ConditionValue<'t, 'py> = Result<Option<Content<'t, 'py>>, PyRenderError>;

impl IfCondition {
    /// Django's `condition.eval(context)`. A variable gives its value (`None`
    /// when missing) and lets its exceptions escape. An operator catches
    /// every `Exception` its operands and its own work raise, and is then
    /// `False`.
    fn value<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ConditionValue<'t, 'py> {
        if let Self::Variable(variable) = self {
            return variable.resolve(
                py,
                template,
                context,
                ResolveFailures::IgnoreVariableDoesNotExist,
            );
        }
        match self.operator_value(py, template, context) {
            Err(error) if operator_catches(py, &error) => Ok(Some(Content::Bool(false))),
            result => result,
        }
    }

    /// The operands of a binary operator, left first.
    fn operands<'t, 'py>(
        pair: &(Self, Self),
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<(Option<Content<'t, 'py>>, Option<Content<'t, 'py>>), PyRenderError> {
        let left = pair.0.value(py, template, context)?;
        let right = pair.1.value(py, template, context)?;
        Ok((left, right))
    }

    fn operator_value<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ConditionValue<'t, 'py> {
        let result = match self {
            Self::Variable(_) => unreachable!("`value` resolves variables"),
            // `x and y` and `x or y` give one of their operands, and only
            // evaluate `y` when `x` doesn't decide the result.
            Self::And(inner) => {
                let left = inner.0.value(py, template, context)?;
                return if truth(left.as_ref())? {
                    inner.1.value(py, template, context)
                } else {
                    Ok(left)
                };
            }
            Self::Or(inner) => {
                let left = inner.0.value(py, template, context)?;
                return if truth(left.as_ref())? {
                    Ok(left)
                } else {
                    inner.1.value(py, template, context)
                };
            }
            Self::Not(inner) => !truth(inner.value(py, template, context)?.as_ref())?,
            Self::Equal(inner) => {
                let (left, right) = Self::operands(inner, py, template, context)?;
                left.compare(&right, CompareOp::Eq)
            }
            Self::NotEqual(inner) => {
                let (left, right) = Self::operands(inner, py, template, context)?;
                left.compare(&right, CompareOp::Ne)
            }
            Self::LessThan(inner) => {
                let (left, right) = Self::operands(inner, py, template, context)?;
                left.compare(&right, CompareOp::Lt)
            }
            Self::GreaterThan(inner) => {
                let (left, right) = Self::operands(inner, py, template, context)?;
                left.compare(&right, CompareOp::Gt)
            }
            Self::LessThanEqual(inner) => {
                let (left, right) = Self::operands(inner, py, template, context)?;
                left.compare(&right, CompareOp::Le)
            }
            Self::GreaterThanEqual(inner) => {
                let (left, right) = Self::operands(inner, py, template, context)?;
                left.compare(&right, CompareOp::Ge)
            }
            Self::In(inner) => match Self::operands(inner, py, template, context)? {
                (left, Some(right)) => right.contains(left).unwrap_or(false),
                _ => false,
            },
            Self::NotIn(inner) => match Self::operands(inner, py, template, context)? {
                (left, Some(right)) => !(right.contains(left).unwrap_or(true)),
                _ => false,
            },
            Self::Is(inner) => {
                let (left, right) = Self::operands(inner, py, template, context)?;
                is_same(py, left.as_ref(), right.as_ref())
            }
            Self::IsNot(inner) => {
                let (left, right) = Self::operands(inner, py, template, context)?;
                !is_same(py, left.as_ref(), right.as_ref())
            }
        };
        Ok(Some(Content::Bool(result)))
    }
}

/// A native int as the object Python would hold. Python keeps one object
/// for each small int, so `x is 5` can be true; an int outside that range is
/// a new object.
fn int_object<'py>(py: Python<'py>, number: &BigInt) -> Bound<'py, PyAny> {
    match i64::try_from(number) {
        Ok(number) => number
            .into_pyobject(py)
            .expect("An i64 can always be converted to a Python int.")
            .into_any(),
        Err(_) => number
            .into_pyobject(py)
            .expect("A BigInt can always be converted to a Python int.")
            .into_any(),
    }
}

/// Python's `left is right`. A missing variable is `None`. Values Rusty
/// computed are the same object as `True` or `False`, or as an int Python
/// shares; its strings and floats are new objects, as Django's are.
fn is_same(
    py: Python<'_>,
    left: Option<&Content<'_, '_>>,
    right: Option<&Content<'_, '_>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(Content::Py(left)), Some(Content::Py(right))) => left.is(right),
        (Some(Content::Py(object)), None) | (None, Some(Content::Py(object))) => object.is_none(),
        (Some(Content::Bool(left)), Some(Content::Bool(right))) => left == right,
        (Some(Content::Bool(value)), Some(Content::Py(object)))
        | (Some(Content::Py(object)), Some(Content::Bool(value))) => {
            object.is(PyBool::new(py, *value).as_any())
        }
        (Some(Content::Int(left)), Some(Content::Int(right))) => {
            left == right && int_object(py, left).is(int_object(py, right))
        }
        (Some(Content::Int(number)), Some(Content::Py(object)))
        | (Some(Content::Py(object)), Some(Content::Int(number))) => {
            object.is(int_object(py, number))
        }
        _ => false,
    }
}

impl Evaluate for IfCondition {
    /// Django's `IfNode`: a condition that raises `VariableDoesNotExist`
    /// doesn't match, and the truth of its value is tested outside the
    /// operators, so an exception from `__bool__` there escapes.
    fn evaluate(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
    ) -> Result<bool, PyRenderError> {
        match self.value(py, template, context) {
            Ok(value) => Ok(truth(value.as_ref())?),
            Err(PyRenderError::RenderError(
                RenderError::VariableDoesNotExist { .. } | RenderError::ArgumentDoesNotExist { .. },
            )) => Ok(false),
            Err(PyRenderError::PyErr(error))
                if error.is_instance_of::<VariableDoesNotExist>(py) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}

/// A Python `int` or `float` that converts to a float exactly, the only
/// values `native_widthratio` takes.
fn exact_float(value: &Bound<'_, PyAny>) -> Option<f64> {
    if value.is_exact_instance_of::<PyFloat>() {
        return value.extract().ok();
    }
    if value.is_exact_instance_of::<PyInt>() {
        let number: i64 = value.extract().ok()?;
        if number.unsigned_abs() <= 1 << 53 {
            #[allow(clippy::cast_precision_loss)]
            return Some(number as f64);
        }
    }
    None
}

/// Django's `round((float(value) / float(max_value)) * int(max_width))` for
/// plain numbers, with the same float operations. `None` leaves strings,
/// subclasses, huge integers and results `round` rejects to Python.
fn native_widthratio(values: &[Bound<'_, PyAny>]) -> Option<String> {
    if !values[2].is_exact_instance_of::<PyInt>() {
        return None;
    }
    let value = exact_float(&values[0])?;
    let maximum = exact_float(&values[1])?;
    let width = exact_float(&values[2])?;
    if maximum == 0.0 {
        return Some("0".to_string());
    }
    let ratio = (value / maximum) * width;
    if !ratio.is_finite() {
        return None;
    }
    BigInt::from_f64(ratio.round_ties_even()).map(|rounded| rounded.to_string())
}

/// Python's `str.isspace`, which `re`'s `\s` and `str.strip` use: Unicode
/// whitespace plus the four information separators.
fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Django's `strip_spaces_between_tags(value.strip())`: `re.sub(r">\s+<",
/// "><", value)`.
fn spaceless(text: &str) -> String {
    let mut rest = text.trim_matches(is_python_space);
    let mut output = String::with_capacity(rest.len());
    while let Some(position) = rest.find('>') {
        output.push_str(&rest[..=position]);
        rest = &rest[position + 1..];
        let trimmed = rest.trim_start_matches(is_python_space);
        if trimmed.len() < rest.len() && trimmed.starts_with('<') {
            rest = trimmed;
        }
    }
    output.push_str(rest);
    output
}

fn render_widthratio(
    args: &[TagElement],
    py: Python<'_>,
    template: TemplateString<'_>,
    context: &mut Context,
) -> Result<String, PyRenderError> {
    let values = args
        .iter()
        .map(|arg| {
            arg.resolve(
                py,
                template,
                context,
                ResolveFailures::IgnoreVariableDoesNotExist,
            )
            .map(|value| value.unwrap_or_else(|| "".as_content()).to_py(py))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(result) = native_widthratio(&values) {
        return Ok(result);
    }
    let builtins = BUILTINS
        .get_or_try_init(py, || {
            py.import("builtins")
                .map(|module| module.into_any().unbind())
        })?
        .bind(py);
    let width = match builtins.getattr("int")?.call1((&values[2],)) {
        Ok(width) => width,
        Err(error)
            if error.is_instance_of::<PyValueError>(py)
                || error.is_instance_of::<PyTypeError>(py) =>
        {
            return Err(
                TemplateSyntaxError::new_err("widthratio final argument must be a number").into(),
            );
        }
        Err(error) => return Err(error.into()),
    };
    let compute = || -> PyResult<String> {
        let float = builtins.getattr("float")?;
        let value = float.call1((&values[0],))?;
        let maximum = float.call1((&values[1],))?;
        let ratio = value
            .call_method1("__truediv__", (maximum,))?
            .call_method1("__mul__", (width,))?;
        builtins.getattr("round")?.call1((ratio,))?.str()?.extract()
    };
    match compute() {
        Ok(value) => Ok(value),
        Err(error) if error.is_instance_of::<PyZeroDivisionError>(py) => Ok("0".to_string()),
        Err(error)
            if error.is_instance_of::<PyValueError>(py)
                || error.is_instance_of::<PyTypeError>(py)
                || error.is_instance_of::<PyOverflowError>(py) =>
        {
            Ok(String::new())
        }
        Err(error) => Err(error.into()),
    }
}

fn render_python_node<'py>(
    py: Python<'py>,
    node: &crate::parse::PythonNode,
    context: &mut Context,
) -> PyResult<Bound<'py, PyAny>> {
    let live = context.formatting_context.as_ref().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("Interop regions require a live Django Context")
    })?;
    let output = node
        .0
        .bind(py)
        .call_method1("render_annotated", (live.bind(py),))
        .inspect_err(|error| {
            // Native and Django Origin objects differ, so Django's
            // annotation hook may leave metadata to the owner.
            let culprit = error
                .value(py)
                .getattr("_culprit_node")
                .unwrap_or_else(|_| node.0.bind(py).clone());
            if let Ok(token) = culprit.getattr("token") {
                let _ = error.value(py).setattr("token", token);
            }
        })?;
    context.refresh_formatting_flags(py)?;
    Ok(output)
}

impl Render for Tag {
    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> super::RenderIntoResult {
        match self {
            Self::PythonNode(node) => {
                let value = render_python_node(py, node, context)?;
                super::types::append_python_string(&value, false, output)?;
                Ok(())
            }
            Self::SimpleTag(tag) => tag.render_into(py, template, context, output),
            Self::SimpleBlockTag(tag) => tag.render_into(py, template, context, output),
            Self::For(tag) => tag.render_into(py, template, context, output),
            Self::Include(tag) => tag.render_into(py, template, context, output),
            Self::InclusionTag(tag) => tag.render_into(py, template, context, output),
            Self::If {
                condition,
                truthy,
                falsey,
            } => {
                if condition.evaluate(py, template, context)? {
                    truthy.render_into(py, template, context, output)
                } else {
                    falsey.render_into(py, template, context, output)
                }
            }
            Self::Autoescape { enabled, nodes } => {
                let previous = context.autoescape;
                context.autoescape = enabled.into();
                if let Some(live) = &context.formatting_context {
                    live.bind(py).setattr("autoescape", context.autoescape)?;
                }
                let result = nodes.render_into(py, template, context, output);
                context.autoescape = previous;
                if let Some(live) = &context.formatting_context {
                    live.bind(py).setattr("autoescape", previous)?;
                }
                result
            }
            Self::With { assignments, nodes } => {
                let values = assignments
                    .iter()
                    .map(|(name, value)| {
                        value
                            .resolve(py, template, context, ResolveFailures::Raise)
                            .map(|value| (name, value.unwrap_or_else(|| "".as_content()).to_py(py)))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let pushed = match &context.formatting_context {
                    Some(live) => super::live::push(live.bind(py)).map(|()| true)?,
                    None => false,
                };
                // Django's `with context.push(**values)`: the names, and
                // anything the body assigns, last until the scope closes.
                let values = values
                    .into_iter()
                    .map(|(name, value)| (name.as_str(), value));
                let rendered = context
                    .push_bindings(py, values)
                    .map_err(Into::into)
                    .and_then(|()| nodes.render_into(py, template, context, output));
                context.pop_variables();
                if pushed && let Some(live) = &context.formatting_context {
                    super::live::pop(live.bind(py))?;
                }
                rendered
            }
            Self::Block { name, nodes } => {
                if let Some(blocks) = &context.python_blocks {
                    let value = blocks
                        .bind(py)
                        .get_item(name)?
                        .call_method1("render", (context.django_context(py)?,))?;
                    super::types::append_python_string(&value, false, output)?;
                    Ok(())
                } else {
                    let owner = context
                        .current_template
                        .as_ref()
                        .expect("inside a template")
                        .clone();
                    context.render_block_into(
                        py,
                        super::inheritance::BlockEntry {
                            name: name.clone(),
                            owner,
                            nodes: nodes.clone(),
                        },
                        output,
                    )
                }
            }
            Self::Verbatim(nodes) => nodes.render_into(py, template, context, output),
            _ => {
                output.push_str(&self.render(py, template, context)?);
                Ok(())
            }
        }
    }

    #[expect(clippy::too_many_lines, reason = "one match arm per builtin tag")]
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        Ok(match self {
            Self::Stock { node, at } => {
                node.render(py, template, context).inspect_err(|error| {
                    if let PyRenderError::PyErr(error) = error
                        && !error
                            .value(py)
                            .hasattr("_rusty_token_span")
                            .unwrap_or(false)
                    {
                        let _ = error.value(py).setattr("_rusty_token_span", at);
                    }
                })?
            }
            Self::PartialDef { definition, inline } => {
                if *inline {
                    let owner = context
                        .current_template
                        .as_ref()
                        .expect("inside a template")
                        .clone();
                    Cow::Owned(
                        owner
                            .render_body(py, &definition.nodes, context)?
                            .into_owned(),
                    )
                } else {
                    Cow::Borrowed("")
                }
            }
            Self::Partial { name, at } => {
                let owner = context
                    .current_template
                    .as_ref()
                    .expect("inside a template")
                    .clone();
                let definition = owner.native_partials().get(name).ok_or_else(|| {
                    TemplateSyntaxError::new_err(format!(
                        "Partial '{name}' is not defined in the current template."
                    ))
                    .annotate(py, *at, "here", template)
                })?;
                let body = owner.block_template(definition.nodes.clone());
                Cow::Owned(context.render_partial(py, &body)?.into_owned())
            }
            Self::Block { name, nodes } => {
                if let Some(blocks) = &context.python_blocks {
                    Cow::Owned(
                        blocks
                            .bind(py)
                            .get_item(name)?
                            .call_method1("render", (context.django_context(py)?,))?
                            .extract()?,
                    )
                } else {
                    let owner = context
                        .current_template
                        .as_ref()
                        .expect("inside a template")
                        .clone();
                    context.render_block(
                        py,
                        super::inheritance::BlockEntry {
                            name: name.clone(),
                            owner,
                            nodes: nodes.clone(),
                        },
                    )?
                }
            }
            Self::Extends {
                at,
                parent,
                parent_token,
                relative,
                ..
            } => {
                let parent_value = if let Some(relative) = relative {
                    PyString::new(py, relative).into_any()
                } else {
                    parent
                        .resolve(py, template, context, ResolveFailures::Raise)?
                        .map_or_else(|| PyString::new(py, "").into_any(), |value| value.to_py(py))
                };
                context
                    .render_parent(
                        py,
                        parent_value,
                        parent_token,
                        !matches!(parent, TagElement::Text(_)),
                    )
                    .map_err(|error| match error {
                        PyRenderError::PyErr(error) => {
                            error.annotate(py, *at, "here", template).into()
                        }
                        error => error,
                    })?
            }
            Self::Regroup(group) => {
                let results = PyList::empty(py);
                if let Some(target) = group.target.resolve(
                    py,
                    template,
                    context,
                    ResolveFailures::IgnoreVariableDoesNotExist,
                )? {
                    let target = target.to_py(py);
                    if !target.is_none()
                        && let Some(lookup) = &group.lookup
                        && context.formatting_context.is_none()
                    {
                        let grouped_result = GROUPED_RESULT.import(
                            py,
                            "django.template.defaulttags",
                            "GroupedResult",
                        )?;
                        let mut previous: Option<Bound<'_, PyAny>> = None;
                        let mut items = PyList::empty(py);
                        for item in target.try_iter()? {
                            let item = item?;
                            let key =
                                super::common::resolve_lookup_path(item.clone(), lookup, context)?;
                            let same = match &previous {
                                Some(previous) => key.is(previous) || key.eq(previous)?,
                                None => false,
                            };
                            if !same {
                                items = PyList::empty(py);
                                results.append(grouped_result.call1((&key, &items))?)?;
                                previous = Some(key);
                            }
                            items.append(item)?;
                        }
                    } else if !target.is_none() {
                        let python_context = context.django_context(py)?;
                        let kwargs = PyDict::new(py);
                        kwargs.set_item("ignore_failures", true)?;
                        let grouped_result = GROUPED_RESULT.import(
                            py,
                            "django.template.defaulttags",
                            "GroupedResult",
                        )?;
                        let mut previous: Option<Bound<'_, PyAny>> = None;
                        let mut items = PyList::empty(py);
                        for item in target.try_iter()? {
                            let item = item?;
                            python_context.set_item(&group.asvar, &item)?;
                            let key = group.expression.bind(py).call_method(
                                "resolve",
                                (&python_context,),
                                Some(&kwargs),
                            )?;
                            let same = match &previous {
                                Some(previous) => key.is(previous) || key.eq(previous)?,
                                None => false,
                            };
                            if !same {
                                items = PyList::empty(py);
                                results.append(grouped_result.call1((&key, &items))?)?;
                                previous = Some(key);
                            }
                            items.append(item)?;
                        }
                    }
                }
                context.insert(group.asvar.clone(), results.into_any())?;
                Cow::Borrowed("")
            }
            Self::IfChanged {
                id,
                values,
                truthy,
                falsey,
            } => {
                let mut output = None;
                let comparison = if values.is_empty() {
                    let rendered = truthy.render(py, template, context)?.into_owned();
                    let value = PyString::new(py, &rendered).into_any();
                    output = Some(rendered);
                    value
                } else {
                    let list = PyList::empty(py);
                    for value in values {
                        let value = value.resolve(
                            py,
                            template,
                            context,
                            ResolveFailures::IgnoreVariableDoesNotExist,
                        )?;
                        list.append(
                            value.map_or_else(|| py.None().into_bound(py), |value| value.to_py(py)),
                        )?;
                    }
                    list.into_any()
                };
                if context.has_changed(py, *id, comparison)? {
                    match output {
                        Some(output) if !output.is_empty() => Cow::Owned(output),
                        _ => truthy.render(py, template, context)?,
                    }
                } else {
                    falsey.render(py, template, context)?
                }
            }
            Self::ResetCycle(id) => {
                context.reset_cycle(py, *id)?;
                Cow::Borrowed("")
            }
            Self::With { .. } => {
                let mut output = String::new();
                self.render_into(py, template, context, &mut output)?;
                Cow::Owned(output)
            }
            Self::FilterBlock { filters, nodes } => {
                let output = nodes.render(py, template, context)?;
                let safe = MARK_BLOCK_SAFE
                    .import(py, "django.utils.safestring", "mark_safe")?
                    .call1((output.as_ref(),))?;
                let mut value = Some(Content::Py(safe));
                for filter in filters {
                    value = filter.resolve(value, py, template, context)?;
                }
                // Django returns the filtered value as the node's output, so
                // one that isn't a string fails when the nodes are joined.
                match value {
                    Some(value) => {
                        let value = value.to_py(py);
                        if !value.is_instance_of::<PyString>() {
                            return Err(PyRenderError::InvalidTagResult(value.unbind()));
                        }
                        Cow::Owned(value.str()?.extract::<String>()?)
                    }
                    None => Cow::Borrowed(""),
                }
            }
            Self::Spaceless(nodes) => {
                let output = nodes.render(py, template, context)?;
                Cow::Owned(spaceless(&output))
            }
            Self::Verbatim(nodes) => nodes.render(py, template, context)?,
            Self::Debug => {
                let values = context.as_dict(py)?;
                for name in ["True", "False", "None"] {
                    if values.contains(name)? {
                        values.del_item(name)?;
                    }
                }
                let ctx = DJANGO_CONTEXT
                    .import(py, "django.template.context", "Context")?
                    .call1((values,))?;
                Cow::Owned(
                    DEBUG_NODE
                        .import(py, "django.template.defaulttags", "DebugNode")?
                        .call0()?
                        .call_method1("render", (ctx,))?
                        .extract()?,
                )
            }
            Self::WidthRatio { args, asvar } => {
                let result = render_widthratio(args, py, template, context)?;
                if let Some(name) = asvar {
                    context.insert(name.clone(), PyString::new(py, &result).into_any())?;
                    Cow::Borrowed("")
                } else {
                    Cow::Owned(result)
                }
            }
            Self::Cycle(cycle) => cycle.render(py, template, context)?,
            Self::Autoescape { enabled, nodes } => {
                let autoescape = context.autoescape;
                context.autoescape = enabled.into();
                if let Some(live) = &context.formatting_context {
                    live.bind(py).setattr("autoescape", context.autoescape)?;
                }
                let rendered = nodes
                    .render(py, template, context)
                    .map(|output| Cow::Owned(output.into_owned()));
                context.autoescape = autoescape;
                if let Some(live) = &context.formatting_context {
                    live.bind(py).setattr("autoescape", autoescape)?;
                }
                rendered?
            }
            Self::If {
                condition,
                truthy,
                falsey,
            } => {
                if condition.evaluate(py, template, context)? {
                    truthy.render(py, template, context)?
                } else {
                    falsey.render(py, template, context)?
                }
            }
            Self::For(for_tag) => for_tag.render(py, template, context)?,
            Self::Include(include_tag) => include_tag.render(py, template, context)?,
            Self::Load => Cow::Borrowed(""),
            Self::PythonNode(node) => {
                Cow::Owned(render_python_node(py, node, context)?.extract::<String>()?)
            }
            Self::QueryString(tag) => {
                let mut args = build_args(py, template, context, &tag.args)?;
                let kwargs = build_kwargs(py, template, context, &tag.kwargs)?;
                if args.is_empty()
                    && let Some(output) = native_querystring(py, context, kwargs.as_ref())?
                {
                    return Ok(if let Some(target) = &tag.target_var {
                        context.insert(target.clone(), PyString::new(py, &output).into_any())?;
                        Cow::Borrowed("")
                    } else if context.autoescape {
                        // Only `&` in an encoded query string needs escaping.
                        Cow::Owned(output.replace('&', "&amp;"))
                    } else {
                        Cow::Owned(output)
                    });
                }
                let python_context = context.django_context(py)?;
                if let Some(request) = &context.request {
                    python_context.setattr("request", request.bind(py))?;
                }
                args.push_front(python_context);
                let output = tag
                    .func
                    .bind(py)
                    .call(PyTuple::new(py, args)?, kwargs.as_ref())?;
                if let Some(target) = &tag.target_var {
                    context.insert(target.clone(), output)?;
                    Cow::Borrowed("")
                } else {
                    Content::Py(output).render(context)?
                }
            }
            Self::InclusionTag(tag) => tag.render(py, template, context)?,
            Self::SimpleTag(simple_tag) => simple_tag.render(py, template, context)?,
            Self::SimpleBlockTag(simple_tag) => simple_tag.render(py, template, context)?,
            Self::Url(url) => url.render(py, template, context)?,
            Self::CsrfToken(csrf_token) => csrf_token.render(py, template, context)?,
            Self::Lorem(lorem) => lorem.render(py, template, context)?,
            Self::Comment(_) => Cow::Borrowed(""),
            Self::Now(now) => now.render(py, template, context)?,
            Self::FirstOf(firstof) => firstof.render(py, template, context)?,
            Self::TemplateTag(template_tag) => Cow::Borrowed(template_tag.output()),
        })
    }
}

impl For {
    fn render_python_into(
        &self,
        iterable: &Bound<'_, PyAny>,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        rendered: &mut String,
    ) -> super::RenderIntoResult {
        // Stop at the first error, as `list()` does: an iterator can keep
        // raising forever.
        let mut list: Vec<_> = match iterable.try_iter() {
            Ok(iterator) => match iterator.collect::<PyResult<_>>() {
                Ok(list) => list,
                Err(error) => {
                    let error =
                        error.annotate(py, self.iterable.at, "while iterating this", template);
                    return Err(error.into());
                }
            },
            Err(error) => {
                let error = error.annotate(py, self.iterable.at, "here", template);
                return Err(error.into());
            }
        };
        if list.is_empty() {
            return self.empty.render_into(py, template, context, rendered);
        }
        if self.reversed {
            list.reverse();
        }
        let mut join = super::JoinResults::default();
        let scopes = context.scope_depth();
        context.push_for_loop(py, list.len());
        let result = (|| {
            for (index, values) in list.into_iter().enumerate() {
                context.push_variables(
                    &self.variables.names,
                    self.variables.at,
                    values,
                    self.iterable.at,
                    index,
                    template,
                )?;
                context.sync_loop_context(py, index == 0)?;
                for (body_index, node) in self.body.iter().enumerate() {
                    join.collect(
                        index * self.body.len() + body_index,
                        node.render_into(py, template, context, rendered),
                    )?;
                }
                context.increment_for_loop();
            }
            Ok(())
        })();
        // Leave the loop's scope on errors too: a caller that catches the
        // error may keep rendering with this context, as Django's does.
        // One scope, as Django pops one layer: if a tag in the body left
        // its own open, that is the one that goes.
        context.pop_scope_above(scopes);
        context.pop_for_loop();
        result.and_then(|()| join.finish(py))
    }

    fn render_string_into(
        &self,
        string: &str,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        rendered: &mut String,
    ) -> super::RenderIntoResult {
        if string.is_empty() {
            return self.empty.render_into(py, template, context, rendered);
        }
        if self.variables.names.len() > 1 {
            return Err(RenderError::TupleUnpackError {
                expected_count: self.variables.names.len(),
                actual_count: 1,
                expected_at: self.variables.at.into(),
                actual_at: self.iterable.at.into(),
            }
            .into());
        }
        let length = string.chars().count();
        let chars = if self.reversed {
            Either::Right(string.chars().rev())
        } else {
            Either::Left(string.chars())
        };
        let variable = &self.variables.names[0];
        let mut join = super::JoinResults::default();
        let mut utf8 = [0; 4];
        let scopes = context.scope_depth();
        context.push_for_loop(py, length);
        let result = (|| {
            for (index, c) in chars.enumerate() {
                let c = PyString::new(py, c.encode_utf8(&mut utf8));
                context.push_variable(variable.clone(), c.into_any(), index);
                context.sync_loop_context(py, index == 0)?;
                for (body_index, node) in self.body.iter().enumerate() {
                    join.collect(
                        index * self.body.len() + body_index,
                        node.render_into(py, template, context, rendered),
                    )?;
                }
                context.increment_for_loop();
            }
            Ok(())
        })();
        context.pop_scope_above(scopes);
        context.pop_for_loop();
        result.and_then(|()| join.finish(py))
    }
}

impl Render for For {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let mut output = String::new();
        self.render_into(py, template, context, &mut output)?;
        Ok(Cow::Owned(output))
    }

    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> super::RenderIntoResult {
        if self.callback_free
            && context.formatting_context.is_some()
            && let Some(result) = context.shadowed(py, |context| {
                self.render_loop_into(py, template, context, output)
            })?
        {
            return result;
        }
        // Keep no handle on the live context or its new layer while the loop
        // renders: an inner loop may then render on native scopes.
        let pushed = match &context.formatting_context {
            Some(live) => super::live::push(live.bind(py)).map(|()| true)?,
            None => false,
        };
        let result = self.render_loop_into(py, template, context, output);
        if pushed && let Some(live) = &context.formatting_context {
            super::live::pop(live.bind(py))?;
        }
        result
    }
}

impl For {
    fn render_loop_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> super::RenderIntoResult {
        let Some(iterable) = self.iterable.iterable.resolve(
            py,
            template,
            context,
            ResolveFailures::IgnoreVariableDoesNotExist,
        )?
        else {
            return self.empty.render_into(py, template, context, output);
        };
        match iterable {
            // Django's `{% for %}` treats `None` as an empty sequence.
            Content::Py(iterable) if iterable.is_none() => {
                self.empty.render_into(py, template, context, output)
            }
            Content::Py(iterable) => {
                self.render_python_into(&iterable, py, template, context, output)
            }
            Content::String(s) => {
                self.render_string_into(s.as_raw(), py, template, context, output)
            }
            value @ (Content::Float(_) | Content::Int(_) | Content::Bool(_)) => {
                self.render_python_into(&value.to_py(py), py, template, context, output)
            }
        }
    }
}

enum IncludeTemplate<'py> {
    Template(Gc<Template>),
    Callable(Bound<'py, PyAny>),
}

impl<'t, 'py> IncludeTemplate<'py> {
    fn render(
        &'t self,
        py: Python<'py>,
        context: &mut Context,
        at: At,
        template: TemplateString<'t>,
    ) -> RenderResult<'t> {
        match self {
            Self::Template(template) => {
                let mut output = String::with_capacity(template.template.len());
                Template::render_shared_into(template, py, context, &mut output)?;
                Ok(Cow::Owned(output))
            }
            Self::Callable(callable) => {
                let py_context = build_pycontext(py, context)?;
                let result = callable.call1((py_context.clone(),));
                retrieve_context(py, py_context, context);
                match result {
                    Ok(content) => Ok(Cow::Owned(content.to_string())),
                    Err(error) => Err(error.annotate(py, at, "here", template).into()),
                }
            }
        }
    }

    fn render_into(
        &'t self,
        py: Python<'py>,
        context: &mut Context,
        at: At,
        template: TemplateString<'t>,
        output: &mut String,
    ) -> super::RenderIntoResult {
        match self {
            Self::Template(template) => Template::render_shared_into(template, py, context, output),
            Self::Callable(_) => {
                output.push_str(&self.render(py, context, at, template)?);
                Ok(())
            }
        }
    }
}

impl Include {
    fn template_at(&self) -> At {
        match &self.template_name {
            IncludeTemplateName::Text(text) => text.at,
            IncludeTemplateName::Variable(TagElement::Variable(variable)) => variable.at,
            IncludeTemplateName::Variable(TagElement::Filter(filter)) => filter.all_at,
            IncludeTemplateName::Variable(TagElement::ForVariable(variable)) => variable.at,
            IncludeTemplateName::Relative(relative) => relative.at,
            IncludeTemplateName::Variable(_) => unreachable!(),
        }
    }

    fn invalid_template_name(
        &self,
        py: Python<'_>,
        content: &str,
        template: TemplateString<'_>,
    ) -> PyRenderError {
        PyTypeError::new_err("Included template name must be a string or iterable of strings.")
            .annotate(
                py,
                self.template_at(),
                &format!("invalid template name: {content}"),
                template,
            )
            .into()
    }

    fn resolve_template_name<'t, 'py>(
        &'t self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<Content<'t, 'py>, PyRenderError> {
        let template_name = match &self.template_name {
            IncludeTemplateName::Text(text) => text
                .resolve(py, template, context, ResolveFailures::Raise)
                .expect("Text should always be resolvable"),
            IncludeTemplateName::Variable(tag_element) => {
                tag_element.resolve(py, template, context, ResolveFailures::Raise)?
            }
            IncludeTemplateName::Relative(relative) => Some(Content::String(
                ContentString::String(Cow::Borrowed(&relative.path)),
            )),
        };
        match template_name {
            Some(template_name) => Ok(template_name),
            None => {
                let error = TemplateDoesNotExist::new_err("No template names provided").annotate(
                    py,
                    self.template_at(),
                    "This variable is not in the context",
                    template,
                );
                Err(error.into())
            }
        }
    }

    fn get_template<'t, 'py>(
        &self,
        template_name: Content<'t, 'py>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &'t mut Context,
    ) -> Result<IncludeTemplate<'py>, PyRenderError> {
        match template_name {
            Content::String(content) => {
                let key = IncludeTemplateKey::String(content.content().to_string());
                context
                    .get_or_insert_include(py, &self.engine, &key, self.id)
                    .map(IncludeTemplate::Template)
            }
            Content::Py(content) => {
                if !content.is_truthy()? {
                    return Err(TemplateDoesNotExist::new_err("No template names provided")
                        .annotate(
                            py,
                            self.template_at(),
                            "This variable is not in the context",
                            template,
                        )
                        .into());
                }
                if let Some(render) = content.getattr_opt(intern!(py, "render"))?
                    && render.is_callable()
                {
                    Ok(IncludeTemplate::Callable(render))
                } else if content.is_instance_of::<PyString>() {
                    let template_path = content
                        .extract()
                        .expect("PyString should be compatible with Cow<str>");
                    let template_path = match construct_relative_path(
                        template_path,
                        self.origin.as_deref(),
                        self.template_at(),
                    )
                    .map_err(RenderError::from)?
                    {
                        Some(path) => path.to_string(),
                        None => template_path.to_string(),
                    };
                    let key = IncludeTemplateKey::String(template_path);
                    context
                        .get_or_insert_include(py, &self.engine, &key, self.id)
                        .map(IncludeTemplate::Template)
                } else {
                    let promise = PROMISE.import(py, "django.utils.functional", "Promise")?;
                    if content.is_instance(promise)? {
                        return Err(PyTypeError::new_err(
                            "Included template name cannot be a translatable string.",
                        )
                        .annotate(
                            py,
                            self.template_at(),
                            &format!("invalid template name: {content:?}"),
                            template,
                        )
                        .into());
                    }
                    let Ok(templates) = content.extract::<Vec<String>>() else {
                        return Err(self.invalid_template_name(
                            py,
                            &format!("{content}"),
                            template,
                        ));
                    };
                    let key = IncludeTemplateKey::Vec(templates);
                    context
                        .get_or_insert_include(py, &self.engine, &key, self.id)
                        .map(IncludeTemplate::Template)
                }
            }
            Content::Int(content) => {
                return Err(self.invalid_template_name(py, &format!("{content}"), template));
            }
            Content::Float(content) => {
                return Err(self.invalid_template_name(py, &format!("{content}"), template));
            }
            Content::Bool(true) => return Err(self.invalid_template_name(py, "True", template)),
            Content::Bool(false) => return Err(self.invalid_template_name(py, "False", template)),
        }
        .map_err(|error| {
            error
                .annotate(py, self.template_at(), "here", template)
                .into()
        })
    }
}

impl Include {
    fn with_include_context<T>(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        render: impl FnOnce(&mut Context) -> Result<T, PyRenderError>,
    ) -> Result<T, PyRenderError> {
        match self.only {
            false => {
                let mut names = Vec::new();
                let mut values = Vec::new();
                for (at, element) in &self.kwargs {
                    let key = template.content(*at);
                    names.push(key);

                    match element.resolve(
                        py,
                        template,
                        context,
                        ResolveFailures::IgnoreVariableDoesNotExist,
                    )? {
                        Some(value) => values.push(value.to_py(py)),
                        None => values.push(PyString::new(py, "").into_any()),
                    }
                }
                let scope = if let Some(live) = &context.formatting_context {
                    Some(live.bind(py).call_method0("push")?)
                } else {
                    None
                };
                context.push_scope(&names);
                for (key, value) in names.iter().zip(values) {
                    if let Some(live) = &context.formatting_context {
                        live.bind(py).set_item(key, &value)?;
                    }
                    context.append(key.to_string(), value);
                }
                let rendered = render(context);
                context.pop_variables();
                if let Some(scope) = scope {
                    scope.call_method1("__exit__", (py.None(), py.None(), py.None()))?;
                }
                rendered
            }
            true => {
                // Django's `Context.new()` starts from the built-in names.
                let mut inner_context = HashMap::from([
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
                for (at, element) in &self.kwargs {
                    let key = template.content(*at).to_string();
                    if let Some(value) = element.resolve(
                        py,
                        template,
                        context,
                        ResolveFailures::IgnoreVariableDoesNotExist,
                    )? {
                        inner_context.insert(key, value.to_py(py).unbind());
                    }
                }
                let request = context
                    .request
                    .as_ref()
                    .map(|request| request.clone_ref(py));
                let mut new_context = Context::new(inner_context, request, context.autoescape);
                new_context.use_l10n = context.use_l10n;
                new_context.use_tz = context.use_tz;
                new_context.format_values = context.format_values;
                new_context.render_stack = context.render_stack;
                new_context.number_format = std::sync::Arc::clone(&context.number_format);
                // Lend the caches: the isolated context has the same
                // language and time zone, and an include in a loop would
                // otherwise fill them again on every call.
                context.move_caches_to(&mut new_context);
                let rendered = render(&mut new_context);
                new_context.move_caches_to(context);
                rendered
            }
        }
    }
}

impl Render for Include {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let template_name = self.resolve_template_name(py, template, context)?;
        let include = self.get_template(template_name, py, template, context)?;
        self.with_include_context(py, template, context, |context| {
            include
                .render(py, context, self.template_at(), template)
                .map(|content| Cow::Owned(content.into_owned()))
        })
    }

    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> super::RenderIntoResult {
        let template_name = self.resolve_template_name(py, template, context)?;
        let include = self.get_template(template_name, py, template, context)?;
        self.with_include_context(py, template, context, |context| {
            include.render_into(py, context, self.template_at(), template, output)
        })
    }
}

fn call_tag<'py>(
    py: Python<'py>,
    func: &Gc<Py<PyAny>>,
    at: At,
    template: TemplateString<'_>,
    args: VecDeque<Bound<'py, PyAny>>,
    kwargs: Option<Bound<'py, PyDict>>,
) -> Result<Bound<'py, PyAny>, PyRenderError> {
    let func = func.bind(py);
    match func.call(
        PyTuple::new(py, args).expect("All arguments should be valid Python objects"),
        kwargs.as_ref(),
    ) {
        Ok(content) => Ok(content),
        Err(error) => Err(error.annotate(py, at, "here", template).into()),
    }
}

/// Spare `PyContext` wrappers, reused so each `takes_context` call doesn't
/// allocate and free one. A spare holds an empty placeholder `Context`.
///
/// Reuse needs proof that Python kept no reference to the wrapper. With the
/// GIL, a reference count of one is that proof; free-threaded Python can't
/// give it, so those builds always allocate.
#[cfg(not(Py_GIL_DISABLED))]
mod spare_contexts {
    use std::cell::RefCell;

    use pyo3::prelude::*;

    use super::PyContext;

    /// Enough for nested callbacks; more would only hold memory.
    const LIMIT: usize = 4;

    thread_local! {
        static SPARES: RefCell<Vec<Py<PyContext>>> = const { RefCell::new(Vec::new()) };
    }

    pub fn take() -> Option<Py<PyContext>> {
        SPARES.with(|spares| spares.borrow_mut().pop())
    }

    pub fn give_back(wrapper: Py<PyContext>) {
        SPARES.with(|spares| {
            let mut spares = spares.borrow_mut();
            if spares.len() < LIMIT {
                spares.push(wrapper);
            }
        });
    }
}

pub(super) fn build_pycontext<'py>(
    py: Python<'py>,
    context: &mut Context,
) -> PyResult<Bound<'py, PyAny>> {
    if let Some(live) = &context.formatting_context {
        return Ok(live.bind(py).clone());
    }
    context.enter_callback();
    #[cfg(not(Py_GIL_DISABLED))]
    if let Some(wrapper) = spare_contexts::take() {
        let wrapper = wrapper.into_bound(py);
        // The spare's placeholder takes the render's place until the call returns.
        std::mem::swap(
            &mut *wrapper
                .borrow()
                .context
                .lock_py_attached(py)
                .expect("Mutex should not be poisoned"),
            context,
        );
        return Ok(wrapper.into_any());
    }
    // Take ownership of `context` so we can pass it to Python.
    // The `context` variable now points to an empty `Context` instance which will not be
    // used except as a placeholder.
    let swapped_context = std::mem::take(context);

    // Wrap the context as a Python object
    Ok(Bound::new(py, PyContext::new(swapped_context))?.into_any())
}

pub(super) fn retrieve_context<'py>(
    py: Python<'py>,
    py_context: Bound<'py, PyAny>,
    context: &mut Context,
) {
    if context.formatting_context.is_some() {
        return;
    }
    #[cfg(not(Py_GIL_DISABLED))]
    {
        let wrapper = py_context
            .cast::<PyContext>()
            .expect("The type of py_context should not have changed");
        // Only this function holds the wrapper, and no block callback holds
        // its state: swap the render back and keep the wrapper for next time.
        // SAFETY: `wrapper` is a live object, and reading a reference count is
        // only a load. Reuse is sound because, on GIL builds (this block is
        // compiled only there), no other thread can change the count while we
        // hold the GIL: a count of 1 is our own handle, so no Python object,
        // callback or retained reference can still reach the wrapper and
        // observe its state being swapped out or the wrapper being pooled.
        // Native handles (block callbacks) are ruled out by
        // `is_sole_native_owner`.
        let refcount = unsafe { pyo3::ffi::Py_REFCNT(wrapper.as_ptr()) };
        if refcount == 1 && wrapper.borrow().context.is_sole_native_owner() {
            std::mem::swap(
                &mut *wrapper
                    .borrow()
                    .context
                    .lock_py_attached(py)
                    .expect("Mutex should not be poisoned"),
                context,
            );
            spare_contexts::give_back(wrapper.clone().unbind());
            context.leave_callback();
            return;
        }
    }
    // Retrieve the PyContext wrapper from Python
    let extracted_context: PyContext = py_context
        .extract()
        .expect("The type of py_context should not have changed");
    // Ensure we only hold one reference in Rust by dropping the Python object.
    drop(py_context);

    // Move unretained state back; retained callbacks keep their GC-visible
    // snapshot and need a copy instead.
    let inner_context = extracted_context
        .context
        .take_or_clone(py, |context| context.clone_ref(py));
    // Put the Context back in `context`
    let _ = std::mem::replace(context, inner_context);
    context.leave_callback();
}

fn build_arg<'py>(
    py: Python<'py>,
    template: TemplateString<'_>,
    context: &mut Context,
    arg: &TagElement,
) -> Result<Bound<'py, PyAny>, PyRenderError> {
    let arg = match arg.resolve(py, template, context, ResolveFailures::Raise)? {
        Some(arg) => arg.to_py(py),
        None => PyString::intern(py, "").into_any(),
    };
    Ok(arg)
}

fn build_args<'py>(
    py: Python<'py>,
    template: TemplateString<'_>,
    context: &mut Context,
    args: &[TagElement],
) -> Result<VecDeque<Bound<'py, PyAny>>, PyRenderError> {
    args.iter()
        .map(|arg| build_arg(py, template, context, arg))
        .collect()
}

fn build_kwargs<'py>(
    py: Python<'py>,
    template: TemplateString<'_>,
    context: &mut Context,
    kwargs: &Vec<(String, TagElement)>,
) -> Result<Option<Bound<'py, PyDict>>, PyRenderError> {
    if kwargs.is_empty() {
        return Ok(None);
    }
    let py_kwargs = PyDict::new(py);
    for (key, value) in kwargs {
        let value = value.resolve(py, template, context, ResolveFailures::Raise)?;
        py_kwargs.set_item(key, value)?;
    }
    Ok(Some(py_kwargs))
}

fn store_target_var_into(
    _py: Python<'_>,
    context: &mut Context,
    content: Bound<'_, PyAny>,
    target_var: Option<&String>,
    rendered: &mut String,
) -> super::RenderIntoResult {
    match target_var {
        None => {
            // `conditional_escape` of a plain `str` is `html.escape`, and of a
            // `SafeString` its own text: both are done here without a call.
            if content.is_exact_instance_of::<PyString>() {
                return Ok(super::types::append_python_string(
                    &content,
                    context.autoescape,
                    rendered,
                )?);
            }
            if crate::render::filters::is_exact_safe_string(&content)? {
                return Ok(super::types::append_python_string(
                    &content, false, rendered,
                )?);
            }
            let output = if context.autoescape {
                CONDITIONAL_ESCAPE
                    .import(content.py(), "django.utils.html", "conditional_escape")?
                    .call1((content,))?
            } else {
                content
            };
            if output.is_instance_of::<PyString>() {
                // Joining uses the underlying Unicode contents even when a
                // string subclass overrides __str__.
                super::types::append_python_string(&output, false, rendered)?;
            } else {
                return Err(PyRenderError::InvalidTagResult(output.unbind()));
            }
            Ok(())
        }
        Some(target_var) => {
            context.insert(target_var.clone(), content)?;
            Ok(())
        }
    }
}

/// The variables `Context.new(values)` gives an inclusion tag's template.
/// Functions usually return a dict, but may return a context.
fn inclusion_values<'py>(
    py: Python<'py>,
    result: Bound<'py, PyAny>,
) -> PyResult<HashMap<String, Py<PyAny>>> {
    if let Ok(values) = result.extract() {
        return Ok(values);
    }
    // `Context.new(None)` is an empty context.
    if result.is_none() {
        return Ok(HashMap::new());
    }
    INCLUSION_VALUES
        .import(py, "django_rusty_templates.stock", "inclusion_values")?
        .call1((result,))?
        .extract()
}

/// One value for a query string key, or `None` when only Django's
/// function knows how to handle it.
fn query_value(value: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    if value.is_instance_of::<PyString>()
        || value.is_exact_instance_of::<PyInt>()
        || value.is_exact_instance_of::<PyFloat>()
        || value.is_exact_instance_of::<PyBool>()
    {
        Ok(Some(value.str()?.to_string()))
    } else {
        Ok(None)
    }
}

/// Like `urllib.parse.quote_plus` with UTF-8.
fn quote_plus(text: &str, output: &mut String) {
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                output.push(char::from(byte));
            }
            b' ' => output.push('+'),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                output.push('%');
                output.push(char::from(HEX[usize::from(byte >> 4)]));
                output.push(char::from(HEX[usize::from(byte & 0xF)]));
            }
        }
    }
}

fn is_utf8(encoding: &Bound<'_, PyAny>) -> bool {
    encoding
        .extract::<String>()
        .is_ok_and(|name| matches!(name.to_ascii_lowercase().as_str(), "utf-8" | "utf8"))
}

/// Django's `{% querystring %}` without positional mappings: the request's
/// `GET` updated with plain keyword values. `None` means Django's function
/// must handle the call, which it then does with identical semantics.
fn native_querystring(
    py: Python<'_>,
    context: &Context,
    kwargs: Option<&Bound<'_, PyDict>>,
) -> PyResult<Option<String>> {
    if context.formatting_context.is_some() {
        return Ok(None);
    }
    let Some(request) = &context.request else {
        return Ok(None);
    };
    let Ok(get) = request.bind(py).getattr(intern!(py, "GET")) else {
        return Ok(None);
    };
    let query_dict = QUERY_DICT.import(py, "django.http", "QueryDict")?;
    if !get.get_type().is(query_dict) || !is_utf8(&get.getattr(intern!(py, "encoding"))?) {
        return Ok(None);
    }
    // Django 6.0 builds a fresh QueryDict, which uses DEFAULT_CHARSET and
    // drops None from lists. Django 5.2 copies `GET` and keeps them.
    let modern = context.django_series(py)? >= (6, 0);
    if modern
        && !is_utf8(
            &QUERY_SETTINGS
                .import(py, "django.conf", "settings")?
                .getattr(intern!(py, "DEFAULT_CHARSET"))?,
        )
    {
        return Ok(None);
    }
    let set = |params: &mut Vec<(String, Vec<String>)>, key: String, values: Vec<String>| {
        if let Some(entry) = params.iter_mut().find(|(name, _)| *name == key) {
            entry.1 = values;
        } else {
            params.push((key, values));
        }
    };
    let list = |value: &Bound<'_, PyAny>| -> PyResult<Option<Vec<String>>> {
        let mut values = Vec::new();
        for item in value.try_iter()? {
            let item = item?;
            if item.is_none() && modern {
                continue;
            }
            let item = if item.is_none() {
                "None".to_string()
            } else if let Some(item) = query_value(&item)? {
                item
            } else {
                return Ok(None);
            };
            values.push(item);
        }
        Ok(Some(values))
    };
    let mut params: Vec<(String, Vec<String>)> = Vec::new();
    let get_dict = get.cast::<PyDict>()?;
    for (key, values) in get_dict.iter() {
        let Ok(key) = key.extract::<String>() else {
            return Ok(None);
        };
        let Some(values) = list(&values)? else {
            return Ok(None);
        };
        set(&mut params, key, values);
    }
    let get_empty = params.is_empty();
    for (key, value) in kwargs.into_iter().flat_map(PyDictMethods::iter) {
        let key: String = key.extract()?;
        if value.is_none() {
            params.retain(|(name, _)| *name != key);
        } else if value.is_exact_instance_of::<PyList>() || value.is_exact_instance_of::<PyTuple>()
        {
            let Some(values) = list(&value)? else {
                return Ok(None);
            };
            set(&mut params, key, values);
        } else if let Some(value) = query_value(&value)? {
            set(&mut params, key, vec![value]);
        } else {
            return Ok(None);
        }
    }
    if !modern && params.is_empty() && get_empty {
        return Ok(Some(String::new()));
    }
    let mut output = String::from("?");
    let mut first = true;
    for (key, values) in &params {
        for value in values {
            if !first {
                output.push('&');
            }
            first = false;
            quote_plus(key, &mut output);
            output.push('=');
            quote_plus(value, &mut output);
        }
    }
    Ok(Some(output))
}

impl InclusionTag {
    fn render_child<'py, T, O>(
        &self,
        py: Python<'py>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut O,
        native: impl FnOnce(&Template, &mut Context, &mut O) -> Result<T, PyRenderError>,
        python: impl FnOnce(Bound<'py, PyAny>, &mut O) -> Result<T, PyRenderError>,
    ) -> Result<T, PyRenderError> {
        let mut args = build_args(py, template, context, &self.call.args)?;
        let kwargs = build_kwargs(py, template, context, &self.call.kwargs)?;
        let func = self.call.func.bind(py);
        // Django passes the live context, so changes the function makes
        // reach later nodes.
        let mut values = if self.call.takes_context {
            let py_context = build_pycontext(py, context)?;
            args.push_front(py_context.clone());
            let result = PyTuple::new(py, args).and_then(|args| func.call(args, kwargs.as_ref()));
            // Read the values before the context moves back to Rust, in case
            // the function returned the context itself.
            let values = result.and_then(|result| inclusion_values(py, result));
            retrieve_context(py, py_context, context);
            values?
        } else {
            inclusion_values(py, func.call(PyTuple::new(py, args)?, kwargs.as_ref())?)?
        };
        if let Some(csrf) = context.resolve_root(py, "csrf_token")?
            && !csrf.is_none()
        {
            values.insert("csrf_token".to_string(), csrf.unbind());
        }
        // Context.new() restores builtins and preserves formatting flags, while
        // excluding the caller's variables, loop state and context processors.
        for (name, value) in [
            ("True", PyBool::new(py, true).to_owned().into_any()),
            ("False", PyBool::new(py, false).to_owned().into_any()),
            ("None", py.None().into_bound(py)),
        ] {
            values
                .entry(name.to_string())
                .or_insert_with(|| value.unbind());
        }
        let mut child = Context::new(
            values,
            context
                .request
                .as_ref()
                .map(|request| request.clone_ref(py)),
            context.autoescape,
        );
        child.use_l10n = context.use_l10n;
        child.use_tz = context.use_tz;
        child.format_values = context.format_values;
        child.render_stack = context.render_stack;
        child.number_format = std::sync::Arc::clone(&context.number_format);
        if let Some(template_name) = self.admin_template {
            // `InclusionAdminNode` lets each model or app override the template.
            let opts = context
                .resolve_root(py, "opts")?
                .ok_or_else(|| PyKeyError::new_err("opts"))?;
            let app_label = opts.getattr("app_label")?.call_method0("lower")?.str()?;
            let object_name = opts.getattr("model_name")?.str()?;
            let key = IncludeTemplateKey::Vec(vec![
                format!("admin/{app_label}/{object_name}/{template_name}"),
                format!("admin/{app_label}/{template_name}"),
                format!("admin/{template_name}"),
            ]);
            let loaded = context.get_or_insert_include(py, &self.engine, &key, self.id)?;
            return native(&loaded, &mut child, output);
        }
        let filename = self.filename.bind(py);
        let filename = filename
            .getattr("template")
            .unwrap_or_else(|_| filename.clone());
        if let Ok(loaded) = filename.extract::<PyRef<'_, Template>>() {
            return native(&loaded, &mut child, output);
        }
        if filename.hasattr("render")? {
            return python(
                filename.call_method1("render", (child.django_context(py)?,))?,
                output,
            );
        }
        let key = if let Ok(name) = filename.extract::<String>() {
            IncludeTemplateKey::String(name)
        } else {
            IncludeTemplateKey::Vec(filename.extract()?)
        };
        let loaded = context.get_or_insert_include(py, &self.engine, &key, self.id)?;
        native(&loaded, &mut child, output)
    }
}

impl Render for InclusionTag {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        self.render_child(
            py,
            template,
            context,
            &mut (),
            |loaded, child, ()| {
                loaded
                    .render(py, child)
                    .map(|output| Cow::Owned(output.into_owned()))
            },
            |output, ()| Ok(Cow::Owned(output.extract()?)),
        )
    }

    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> super::RenderIntoResult {
        self.render_child(
            py,
            template,
            context,
            output,
            |loaded, child, output| loaded.render_into(py, child, output),
            |value, output| {
                super::types::append_python_string(&value, false, output)?;
                Ok(())
            },
        )
    }
}

impl Render for SimpleTag {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let mut output = String::new();
        self.render_into(py, template, context, &mut output)?;
        Ok(Cow::Owned(output))
    }

    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> super::RenderIntoResult {
        let mut args = build_args(py, template, context, &self.args)?;
        let kwargs = build_kwargs(py, template, context, &self.kwargs)?;
        let content = if self.takes_context {
            let py_context = build_pycontext(py, context)?;
            args.push_front(py_context.clone());

            // Actually call the tag
            let result = call_tag(py, &self.func, self.at, template, args, kwargs);

            retrieve_context(py, py_context, context);

            // Return the result of calling the tag
            result?
        } else {
            call_tag(py, &self.func, self.at, template, args, kwargs)?
        };
        store_target_var_into(py, context, content, self.target_var.as_ref(), output)
    }
}

impl Render for SimpleBlockTag {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let mut output = String::new();
        self.render_into(py, template, context, &mut output)?;
        Ok(Cow::Owned(output))
    }

    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> super::RenderIntoResult {
        let mut args = build_args(py, template, context, &self.args)?;
        let kwargs = build_kwargs(py, template, context, &self.kwargs)?;

        let content = self.nodes.render(py, template, context)?;
        let content = MARK_SAFE
            .import(py, "django.utils.safestring", "mark_safe")?
            .call1((&*content,))?;
        args.push_front(content);

        let content = if self.takes_context {
            let py_context = build_pycontext(py, context)?;
            args.push_front(py_context.clone());

            // Actually call the tag
            let result = call_tag(py, &self.func, self.at, template, args, kwargs);

            retrieve_context(py, py_context, context);

            // Return the result of calling the tag
            result?
        } else {
            call_tag(py, &self.func, self.at, template, args, kwargs)?
        };
        store_target_var_into(py, context, content, self.target_var.as_ref(), output)
    }
}

impl Render for Now {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        // Mirrors Django's NowNode: the current time in the current time zone
        // (when USE_TZ is on), formatted by the date filter.
        let (format, escape) = match &self.expression {
            Some(expression) => (
                expression
                    .resolve(
                        py,
                        template,
                        context,
                        ResolveFailures::IgnoreVariableDoesNotExist,
                    )?
                    .unwrap_or_else(|| "".as_content()),
                self.escape && context.autoescape,
            ),
            None => (self.format.as_content(), false),
        };
        let now = context.date_cache.borrow_mut().now(py)?;
        let date = DATE_FILTER.import(py, "django.template.defaultfilters", "date")?;
        let formatted = format_localtime(now, Some(format), Formatter::Date, date, context)?;
        let formatted = match formatted {
            Content::String(formatted) if escape && self.asvar.is_none() => {
                return Ok(Cow::Owned(
                    html_escape::encode_quoted_attribute(formatted.as_raw()).into_owned(),
                ));
            }
            Content::String(formatted) if !escape => {
                let formatted = formatted.into_raw();
                if self.asvar.is_none() {
                    return Ok(Cow::Owned(formatted.into_owned()));
                }
                PyString::new(py, &formatted).into_any()
            }
            formatted => formatted.to_py(py),
        };
        let formatted = match escape {
            true => CONDITIONAL_ESCAPE
                .import(py, "django.utils.html", "conditional_escape")?
                .call1((formatted,))?,
            false => formatted,
        };
        if let Some(at) = self.asvar {
            context.insert(template.content(at).to_string(), formatted)?;
            return Ok(Cow::Borrowed(""));
        }
        Ok(Cow::Owned(
            formatted
                .cast_into::<PyString>()
                .map_err(PyErr::from)?
                .to_str()?
                .to_owned(),
        ))
    }
}

impl CsrfToken {
    const MISSING_WARNING: &str = "A {% csrf_token %} was used in a template, but the context did not provide the value.  This is usually caused by not providing a request.";

    fn input_html(token_str: &str) -> String {
        format!(
            r#"<input type="hidden" name="csrfmiddlewaretoken" value="{}">"#,
            html_escape::encode_quoted_attribute(token_str)
        )
    }
}

impl Render for CsrfToken {
    fn render<'t>(
        &self,
        py: Python<'_>,
        _template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        match context.resolve_root(py, "csrf_token")? {
            Some(bound_token) => {
                if let Ok(token_str) = bound_token.extract::<String>() {
                    if token_str.is_empty() || token_str == "NOTPROVIDED" {
                        Ok(Cow::Borrowed(""))
                    } else {
                        Ok(Cow::Owned(Self::input_html(&token_str)))
                    }
                } else if bound_token.is_truthy()? {
                    let token_py_str = bound_token.str()?;
                    let token_str = token_py_str.to_str()?;
                    Ok(Cow::Owned(Self::input_html(token_str)))
                } else {
                    Ok(Cow::Borrowed(""))
                }
            }
            None => {
                let settings = DJANGO_SETTINGS.import(py, "django.conf", "settings")?;
                let debug = settings.getattr("DEBUG")?.is_truthy()?;

                if debug {
                    let warn = WARNINGS_WARN.import(py, "warnings", "warn")?;
                    warn.call1((Self::MISSING_WARNING,))?;
                }

                Ok(Cow::Borrowed(""))
            }
        }
    }
}

impl Render for Lorem {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let count_content = self.count.resolve(
            py,
            template,
            context,
            ResolveFailures::IgnoreVariableDoesNotExist,
        )?;
        // Django catches `ValueError` and `TypeError` from `int(count)`,
        // but not the `OverflowError` an infinity raises.
        if count_content.as_ref().is_some_and(Content::is_infinite) {
            return Err(
                PyOverflowError::new_err("cannot convert float infinity to integer").into(),
            );
        }
        let val = count_content
            .and_then(|c| c.to_bigint())
            .and_then(|n| n.to_i64())
            .unwrap_or(1);

        let text = match self.method {
            LoremMethod::Words => {
                let final_count = match (val, self.common) {
                    // A negative count drops that many words from the common words.
                    (val, true) if val < 0 => usize::try_from(val.unsigned_abs())
                        .map_or(0, |drop| COMMON_WORDS.len().saturating_sub(drop)),
                    (val, false) if val < 0 => 0,
                    (val, _) => usize::try_from(val).unwrap_or(usize::MAX),
                };
                words(final_count, self.common)
            }
            LoremMethod::Paragraphs | LoremMethod::Blocks => {
                if val <= 0 {
                    return Ok(Cow::Borrowed(""));
                }
                let count = usize::try_from(val).unwrap_or(usize::MAX);
                let paras = paragraphs(count, self.common);
                if matches!(self.method, LoremMethod::Paragraphs) {
                    paras
                        .into_iter()
                        .map(|p| format!("<p>{p}</p>"))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                } else {
                    paras.join("\n\n")
                }
            }
        };

        Ok(Cow::Owned(text))
    }
}

impl Render for FirstOf {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        for var in &self.vars {
            if let Some(content) = var.resolve(
                py,
                template,
                context,
                ResolveFailures::IgnoreVariableDoesNotExist,
            )? && content.to_bool()?
            {
                if let Some(asvar) = &self.asvar {
                    // Django stores `render_value_in_context(value)`. With
                    // autoescape on, that is the escaped output marked safe,
                    // which the native output path already produces.
                    let rendered = if context.autoescape && context.formatting_context.is_none() {
                        let output = content.render(context)?;
                        FIRSTOF_SAFE_STRING
                            .import(py, "django.utils.safestring", "SafeString")?
                            .call1((output.as_ref(),))?
                    } else {
                        RENDER_FIRSTOF_VALUE
                            .import(py, "django.template.base", "render_value_in_context")?
                            .call1((content.to_py(py), context.django_context(py)?))?
                    };
                    context.insert(asvar.clone(), rendered)?;
                    return Ok(Cow::Borrowed(""));
                }

                let format = content.render(context)?.to_string();
                return Ok(Cow::Owned(format));
            }
        }
        if let Some(asvar) = &self.asvar {
            context.insert(asvar.clone(), PyString::new(py, "").into_any())?;
        }
        Ok(Cow::Borrowed(""))
    }
}

struct ResolvedCycleValue<'t, 'py> {
    at: At,
    content: Option<Content<'t, 'py>>,
}

impl SimpleCycle {
    fn resolve_next<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<ResolvedCycleValue<'t, 'py>, PyRenderError> {
        let index = context.next_cycle_index(py, self.id, self.values.len())?;
        let CycleValue { value, at } = &self.values[index];
        let content = value.resolve(py, template, context, ResolveFailures::Raise)?;
        Ok(ResolvedCycleValue { at: *at, content })
    }
}

impl NamedCycle {
    fn resolve_and_store<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> Result<ResolvedCycleValue<'t, 'py>, PyRenderError> {
        let ResolvedCycleValue { at, content } = self.cycle.resolve_next(py, template, context)?;

        if let Some(content) = &content {
            context.insert_cycle(self.asvar.clone(), content.to_py(py))?;
        } else {
            context.insert_cycle(self.asvar.clone(), intern!(py, "").clone().into_any())?;
        }

        Ok(ResolvedCycleValue { at, content })
    }
}

impl Render for SimpleCycle {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let ResolvedCycleValue { at, content } = self.resolve_next(py, template, context)?;
        let Some(content) = content else {
            return Ok(Cow::Borrowed(""));
        };

        Ok(content
            .render(context)
            .map_err(|error| error.annotate(py, at, "here", template))?)
    }
}

impl Render for NamedCycle {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let ResolvedCycleValue { at, content } = self.resolve_and_store(py, template, context)?;
        let Some(content) = content else {
            return Ok(Cow::Borrowed(""));
        };

        Ok(content
            .render(context)
            .map_err(|error| error.annotate(py, at, "here", template))?)
    }
}

impl Render for SilentNamedCycle {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        self.cycle.resolve_and_store(py, template, context)?;
        Ok(Cow::Borrowed(""))
    }
}

impl Render for Cycle {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        match self {
            Self::Simple(cycle) => cycle.render(py, template, context),
            Self::Named(cycle) => cycle.render(py, template, context),
            Self::SilentNamed(cycle) => cycle.render(py, template, context),
        }
    }
}
