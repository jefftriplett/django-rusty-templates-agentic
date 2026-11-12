use std::borrow::Cow;
use std::cell::RefCell;

use pyo3::exceptions::{PyAttributeError, PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyDict, PyFunction, PyList, PyString, PyTuple, PyType};
use rustc_hash::FxHashMap;

use dtl_lexer::types::TemplateString;

use super::types::{AsBorrowedContent, Content, ContentString, Context, render_int};
use super::{Render, RenderResult, Resolve, ResolveFailures, ResolveResult};
use crate::error::{AnnotatePyErr, RenderError};
use crate::parse::{TagElement, TokenTree};
use crate::types::Argument;
use crate::types::ArgumentType;
use crate::types::ForVariable;
use crate::types::ForVariableName;
use crate::types::Text;
use crate::types::TranslatedText;
use dtl_lexer::types::Variable;

static BASE_CONTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DIR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static OBJECT_DIR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SIGNATURE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static METHOD_TYPE: PyOnceLock<Py<PyType>> = PyOnceLock::new();

static GETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Helper function to translate a string using Django's gettext
pub fn gettext(py: Python<'_>, text: &str) -> PyResult<String> {
    let get_text = GETTEXT.import(py, "django.utils.translation", "gettext")?;
    get_text.call1((text,))?.extract::<String>()
}

/// Where Django's `getattr` finds `do_not_call_in_templates` and
/// `alters_data` on a callable.
enum FlagSource<'a, 'py> {
    /// The `__dict__` of a Python function, or of a bound method's function.
    /// Neither type defines the flags and a bound method forwards other
    /// lookups to its function, so `getattr` can only find them here.
    /// Reading the dictionary avoids raising two `AttributeError`s per call.
    FunctionDict(Bound<'py, PyDict>),
    /// Any other callable, whose flags need a full attribute lookup.
    Object(&'a Bound<'py, PyAny>),
}

impl<'a, 'py> FlagSource<'a, 'py> {
    fn new(variable: &'a Bound<'py, PyAny>) -> PyResult<Self> {
        let py = variable.py();
        let function = if variable
            .get_type()
            .is(METHOD_TYPE.import(py, "types", "MethodType")?)
        {
            variable.getattr(intern!(py, "__func__"))?
        } else {
            variable.clone()
        };
        if !function.is_exact_instance_of::<PyFunction>() {
            return Ok(Self::Object(variable));
        }
        Ok(
            match function
                .getattr(intern!(py, "__dict__"))?
                .cast_into_exact::<PyDict>()
            {
                Ok(attributes) => Self::FunctionDict(attributes),
                Err(_) => Self::Object(variable),
            },
        )
    }

    fn is_set(&self, flag: &Bound<'py, PyString>) -> PyResult<bool> {
        let value = match self {
            Self::FunctionDict(attributes) => attributes.get_item(flag)?,
            Self::Object(variable) => variable.getattr_opt(flag)?,
        };
        value.map_or(Ok(false), |value| value.is_truthy())
    }
}

fn resolve_callable<'py>(
    variable: Bound<'py, PyAny>,
    context: &mut Context,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    if !variable.is_callable() {
        return Ok(Some(variable));
    }
    let py = variable.py();
    let flags = FlagSource::new(&variable)?;
    if flags.is_set(intern!(py, "do_not_call_in_templates"))? {
        return Ok(Some(variable));
    }
    if flags.is_set(intern!(py, "alters_data"))? {
        return Ok(None);
    }
    match context.call_variable(py, &variable) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.is_instance_of::<PyTypeError>(py) => {
            let signature = SIGNATURE
                .import(py, "django.utils.inspect", "signature")?
                .call1((&variable,));
            match signature {
                Err(signature_error) if signature_error.is_instance_of::<PyValueError>(py) => {
                    Ok(None)
                }
                Err(signature_error) => Err(signature_error),
                Ok(signature) => match signature.call_method0("bind") {
                    Err(bind_error) if bind_error.is_instance_of::<PyTypeError>(py) => Ok(None),
                    Err(bind_error) => Err(bind_error),
                    Ok(_) => Err(error),
                },
            }
        }
        Err(error) => Err(error),
    }
}

/// Most names a thread can keep. Templates repeat a small set of names,
/// so this is only reached by a process generating templates without end.
const MAX_PART_NAMES: usize = 1 << 16;

thread_local! {
    /// Python's interned string for each name looked up on this thread.
    static PART_NAMES: RefCell<FxHashMap<Box<str>, Py<PyString>>> = RefCell::default();
}

/// The interned Python string for a variable name or lookup part.
///
/// Building a `str` per lookup costs an allocation and a hash each time.
/// An interned one keeps its hash, and attribute lookups only use the type's
/// method cache for interned names.
pub(crate) fn interned_name<'py>(py: Python<'py>, part: &str) -> Bound<'py, PyString> {
    PART_NAMES.with_borrow_mut(|names| {
        if let Some(name) = names.get(part) {
            return name.bind(py).clone();
        }
        let name = PyString::intern(py, part);
        if names.len() < MAX_PART_NAMES {
            names.insert(part.into(), name.clone().unbind());
        }
        name
    })
}

/// Whether `variable[key]` can work at all: its type has a mapping or a
/// sequence item slot, which is where Python stores `__getitem__`.
fn is_subscriptable(variable: &Bound<'_, PyAny>) -> bool {
    // SAFETY: `variable` is a valid object and the GIL is held.
    let mapping = unsafe { pyo3::ffi::PyMapping_Check(variable.as_ptr()) };
    if mapping != 0 {
        return true;
    }
    // SAFETY: as above.
    let sequence = unsafe { pyo3::ffi::PySequence_Check(variable.as_ptr()) };
    sequence != 0
}

/// `name in dir(variable)`, which Django checks to re-raise an
/// `AttributeError` from an attribute that exists, such as a property.
///
/// With the default `object.__dir__`, `dir()` lists the instance `__dict__`
/// and every `__dict__` along the type's MRO, so those are checked directly
/// rather than building and sorting the whole list.
fn in_dir(variable: &Bound<'_, PyAny>, name: &Bound<'_, PyString>) -> PyResult<bool> {
    let py = variable.py();
    let object_dir = OBJECT_DIR.get_or_try_init(py, || {
        py.get_type::<PyAny>()
            .getattr(intern!(py, "__dir__"))
            .map(Bound::unbind)
    })?;
    let class = variable.get_type();
    let default_dir = class
        .getattr(intern!(py, "__dir__"))
        .is_ok_and(|dir| dir.is(object_dir.bind(py)));
    // A proxy such as `SimpleLazyObject` reports another `__class__`, which
    // `object.__dir__` follows.
    let own_class = variable
        .getattr(intern!(py, "__class__"))
        .is_ok_and(|reported| reported.is(&class));
    if !default_dir || !own_class {
        return DIR
            .import(py, "builtins", "dir")?
            .call1((variable,))?
            .contains(name);
    }
    if let Ok(Some(attributes)) = variable.getattr_opt(intern!(py, "__dict__"))
        && let Ok(attributes) = attributes.cast_into::<PyDict>()
        && attributes.contains(name)?
    {
        return Ok(true);
    }
    for base in class.mro().iter() {
        if base.getattr(intern!(py, "__dict__"))?.contains(name)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn lookup_part<'py>(
    variable: Bound<'py, PyAny>,
    part: &str,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let py = variable.py();
    // `list["0"]` raises TypeError and an exact list or tuple can have no
    // attribute named by digits, so Django ends up indexing it. Go there
    // directly instead of raising and catching two errors and calling dir().
    if (variable.is_exact_instance_of::<PyList>() || variable.is_exact_instance_of::<PyTuple>())
        && let Ok(index) = part.parse::<isize>()
    {
        return match variable.get_item(index) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.is_instance_of::<PyIndexError>(py) => Ok(None),
            Err(error) => Err(error),
        };
    }
    let name = interned_name(py, part);
    let name = &name;
    // Exact dictionaries need no generic mapping dispatch or KeyError on a
    // miss. Missing keys must still fall through to attributes and integer keys.
    let exact_dict = variable.cast_exact::<PyDict>().ok();
    if let Some(dict) = &exact_dict {
        if let Some(value) = dict.get_item(name)? {
            return Ok(Some(value));
        }
    } else if is_subscriptable(&variable) {
        match variable.get_item(name) {
            Ok(value) => return Ok(Some(value)),
            Err(error)
                if error.is_instance_of::<PyTypeError>(py)
                    || error.is_instance_of::<PyAttributeError>(py)
                    || error.is_instance_of::<PyKeyError>(py)
                    || error.is_instance_of::<PyValueError>(py)
                    || error.is_instance_of::<PyIndexError>(py) => {}
            Err(error) => return Err(error),
        }
    }
    let is_context = if exact_dict.is_some() {
        false
    } else {
        // A type check, not `isinstance`: for every object that isn't a
        // context, `isinstance` also reads `__class__`, which was most of the
        // cost of a lookup. Only an object whose `__class__` claims to be a
        // context without its type being one would differ.
        let base_context = BASE_CONTEXT.import(py, "django.template.context", "BaseContext")?;
        variable.get_type().is_subclass(base_context)?
    };
    if !is_context || !variable.get_type().hasattr(name)? {
        match variable.getattr(name) {
            Ok(value) => return Ok(Some(value)),
            Err(error)
                if error.is_instance_of::<PyTypeError>(py)
                    || error.is_instance_of::<PyAttributeError>(py) =>
            {
                // A built-in dict has no user-defined property that could
                // raise AttributeError. Only inspect dir() for other objects.
                if !is_context && exact_dict.is_none() && in_dir(&variable, name)? {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
    let Ok(index) = part.parse::<isize>() else {
        return Ok(None);
    };
    match variable.get_item(index) {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if error.is_instance_of::<PyTypeError>(py)
                || error.is_instance_of::<PyKeyError>(py)
                || error.is_instance_of::<PyIndexError>(py)
                || error.is_instance_of::<PyValueError>(py) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Resolve `object.part.part` like Django's `Variable` resolving
/// `{asvar}.{path}` with `ignore_failures=True`, as `regroup` does for each
/// item. A missing part gives `None`.
pub(crate) fn resolve_lookup_path<'py>(
    object: Bound<'py, PyAny>,
    path: &[String],
    context: &mut Context,
) -> PyResult<Bound<'py, PyAny>> {
    let py = object.py();
    let mut value = match resolve_callable(object, context) {
        Ok(Some(value)) => value,
        Ok(None) => return context.invalid_value(py, None),
        Err(error) if silent_failure(&error, py) => return context.invalid_value(py, None),
        Err(error) => return Err(error),
    };
    for part in path {
        let found = match lookup_part(value, part) {
            Ok(Some(found)) => found,
            // A missing part is VariableDoesNotExist, which regroup ignores.
            Ok(None) => return Ok(py.None().into_bound(py)),
            Err(error) if silent_failure(&error, py) => return context.invalid_value(py, None),
            Err(error) => return Err(error),
        };
        value = match resolve_callable(found, context) {
            Ok(Some(value)) => value,
            // Django substitutes string_if_invalid for a callable it won't
            // call, then keeps looking up the remaining parts on it.
            Ok(None) => context.invalid_value(py, None)?,
            Err(error) if silent_failure(&error, py) => return context.invalid_value(py, None),
            Err(error) => return Err(error),
        };
    }
    Ok(value)
}

fn silent_failure(error: &PyErr, py: Python<'_>) -> bool {
    error
        .value(py)
        .getattr("silent_variable_failure")
        .and_then(|value| value.is_truthy())
        .unwrap_or(false)
}

impl Resolve for Variable {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
        failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        let mut parts = self.parts(template).peekable();
        let (first, mut object_at) = parts.next().expect("Variable names cannot be empty");
        let Some(variable) = context.resolve_root(py, first)? else {
            return Ok(None);
        };
        let variable = if variable.is_instance_of::<super::inheritance::BlockMarker>()
            && parts.peek().is_some_and(|(part, _)| *part == "super")
        {
            parts.next();
            let marker = variable
                .extract::<PyRef<'_, super::inheritance::BlockMarker>>()
                .map_err(PyErr::from)?
                .clone();
            let output = context
                .render_super(py, &marker)
                .map_err(|error| match error {
                    crate::error::PyRenderError::PyErr(error) => {
                        error.annotate(py, self.at, "here", template).into()
                    }
                    error => error,
                })?;
            let content = Content::String(ContentString::HtmlSafe(Cow::Owned(output)));
            if parts.peek().is_none() {
                return Ok(Some(content));
            }
            content.to_py(py)
        } else {
            variable
        };
        let mut variable = match resolve_callable(variable, context) {
            Ok(Some(variable)) => variable,
            Ok(None) => return Ok(Some(Content::Py(context.invalid_value(py, None)?))),
            Err(error) if silent_failure(&error, py) => {
                return Ok(Some(Content::Py(context.invalid_value(py, None)?)));
            }
            Err(error) => return Err(error.annotate(py, self.at, "here", template).into()),
        };
        for (part, key_at) in parts {
            let resolved = lookup_part(variable.clone(), part).and_then(|value| match value {
                Some(value) => resolve_callable(value, context),
                None => Ok(None),
            });
            variable = match resolved {
                Ok(Some(value)) => value,
                Ok(None) => {
                    return match failures {
                        ResolveFailures::Raise => Err(RenderError::VariableDoesNotExist {
                            key: part.to_string(),
                            object: variable.str()?.to_string(),
                            key_at: key_at.into(),
                            object_at: Some(object_at.into()),
                        }
                        .into()),
                        ResolveFailures::IgnoreVariableDoesNotExist => Ok(None),
                    };
                }
                Err(error) if silent_failure(&error, py) => {
                    return Ok(Some(Content::Py(context.invalid_value(py, None)?)));
                }
                Err(error) => return Err(error.annotate(py, self.at, "here", template).into()),
            };
            object_at.1 += key_at.1 + 1;
        }
        context.refresh_formatting_flags(py)?;
        Ok(Some(Content::Py(variable)))
    }
}

fn resolve_output_variable<'t, 'py>(
    variable: &Variable,
    py: Python<'py>,
    template: TemplateString<'t>,
    context: &mut Context,
) -> ResolveResult<'t, 'py> {
    match variable.resolve(py, template, context, ResolveFailures::Raise) {
        Ok(None)
        | Err(crate::error::PyRenderError::RenderError(RenderError::VariableDoesNotExist {
            ..
        })) => Ok(Some(Content::Py(
            context.invalid_value(py, Some(template.content(variable.at)))?,
        ))),
        result => result,
    }
}

impl Resolve for ForVariable {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        _template: TemplateString<'t>,
        context: &mut Context,
        _failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        if context.formatting_context.is_some()
            || context.loop_dictionary_cached(py, self.parent_count)
        {
            let Some(mut value) = context.resolve_root(py, "forloop")? else {
                return Ok(None);
            };
            for _ in 0..self.parent_count {
                let Some(parent) = lookup_part(value, "parentloop")? else {
                    return Ok(None);
                };
                value = parent;
            }
            let key = match self.variant {
                ForVariableName::Length => Some("length"),
                ForVariableName::Counter => Some("counter"),
                ForVariableName::Counter0 => Some("counter0"),
                ForVariableName::RevCounter => Some("revcounter"),
                ForVariableName::RevCounter0 => Some("revcounter0"),
                ForVariableName::First => Some("first"),
                ForVariableName::Last => Some("last"),
                ForVariableName::Object => None,
            };
            if let Some(key) = key {
                let Some(part) = lookup_part(value, key)? else {
                    return Ok(None);
                };
                value = part;
            }
            let result = resolve_callable(value, context)?.map(Content::Py);
            context.refresh_formatting_flags(py)?;
            return Ok(result);
        }
        // `forloop.length` is new in Django 6.0; before that it is missing.
        if self.variant == ForVariableName::Length && context.django_series(py)? < (6, 0) {
            return Ok(None);
        }
        let Some(for_loop) = context.get_for_loop(self.parent_count) else {
            return Ok(Some("{}".as_content()));
        };
        Ok(Some(match self.variant {
            ForVariableName::Length => Content::Int(for_loop.length().into()),
            ForVariableName::Counter => Content::Int(for_loop.counter().into()),
            ForVariableName::Counter0 => Content::Int(for_loop.counter0().into()),
            ForVariableName::RevCounter => Content::Int(for_loop.rev_counter().into()),
            ForVariableName::RevCounter0 => Content::Int(for_loop.rev_counter0().into()),
            ForVariableName::First => Content::Bool(for_loop.first()),
            ForVariableName::Last => Content::Bool(for_loop.last()),
            ForVariableName::Object => {
                let content = Cow::Owned(context.render_for_loop(py, self.parent_count));
                let content = match context.autoescape {
                    false => ContentString::String(content),
                    true => ContentString::HtmlUnsafe(content),
                };
                Content::String(content)
            }
        }))
    }
}

fn unescape_literal(text: &str) -> Cow<'_, str> {
    if !text.contains('\\') {
        return Cow::Borrowed(text);
    }
    let mut chars = text.chars().peekable();
    let mut output = String::with_capacity(text.len());
    while let Some(character) = chars.next() {
        if character == '\\' && matches!(chars.peek(), Some('\\' | '\'' | '"')) {
            output.push(chars.next().expect("peeked character"));
        } else {
            output.push(character);
        }
    }
    Cow::Owned(output)
}

impl Resolve for Text {
    fn resolve<'t, 'py>(
        &self,
        _py: Python<'py>,
        template: TemplateString<'t>,
        _context: &mut Context,
        _failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        // Django marks string literals safe whatever the autoescape setting.
        Ok(Some(Content::String(ContentString::HtmlSafe(
            unescape_literal(template.content(self.at)),
        ))))
    }
}

impl Resolve for TranslatedText {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        _context: &mut Context,
        _failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        let text = template.content(self.at);
        let resolved = gettext(py, &unescape_literal(text))?;
        // Django translates the literal's safe string, so the result is safe.
        Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
            resolved,
        )))))
    }
}

impl Resolve for Argument {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
        failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        Ok(Some(match &self.argument_type {
            ArgumentType::Text(text) => return text.resolve(py, template, context, failures),
            ArgumentType::TranslatedText(text) => {
                return text.resolve(py, template, context, failures);
            }
            ArgumentType::Variable(variable) => {
                match variable.resolve(py, template, context, failures)? {
                    Some(content) => content,
                    None => {
                        let key = template.content(variable.at).to_string();
                        let object = context.display(py);
                        return Err(RenderError::ArgumentDoesNotExist {
                            key,
                            object,
                            key_at: variable.at.into(),
                            object_at: None,
                        }
                        .into());
                    }
                }
            }
            ArgumentType::ForVariable(variable) => {
                match variable.resolve(py, template, context, failures)? {
                    Some(content) => content,
                    // Such as `forloop.length` before Django 6.0.
                    None => {
                        let key = template.content(variable.at).to_string();
                        let object = context.display(py);
                        return Err(RenderError::ArgumentDoesNotExist {
                            key,
                            object,
                            key_at: variable.at.into(),
                            object_at: None,
                        }
                        .into());
                    }
                }
            }
            ArgumentType::Float(number) => Content::Float(*number),
            ArgumentType::Int(number) => Content::Int(number.clone()),
        }))
    }
}

impl Resolve for TagElement {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
        failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        match self {
            Self::Text(text) => text.resolve(py, template, context, failures),
            Self::TranslatedText(text) => {
                TranslatedText::new(text.at).resolve(py, template, context, failures)
            }
            Self::Variable(variable) => {
                if failures == ResolveFailures::Raise {
                    resolve_output_variable(variable, py, template, context)
                } else {
                    variable.resolve(py, template, context, failures)
                }
            }
            Self::ForVariable(variable) => variable.resolve(py, template, context, failures),
            Self::Filter(filter) => filter.resolve(py, template, context, failures),
            Self::Int(int) => Ok(Some(Content::Int(int.clone()))),
            Self::Float(float) => Ok(Some(Content::Float(*float))),
        }
    }
}

impl Render for TokenTree {
    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> super::RenderIntoResult {
        match self {
            Self::Text(text) => output.push_str(template.content(text.at)),
            Self::Tag(tag) => return tag.render_into(py, template, context, output),
            Self::Variable(variable) => {
                if let Some(value) = resolve_output_variable(variable, py, template, context)? {
                    value.render_into(context, output)?;
                }
            }
            Self::ForVariable(variable) => {
                return variable.render_into(py, template, context, output);
            }
            Self::Filter(filter) => return filter.render_into(py, template, context, output),
            _ => output.push_str(&self.render(py, template, context)?),
        }
        Ok(())
    }

    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        match self {
            Self::Text(text) => Ok(Cow::Borrowed(template.content(text.at))),
            Self::LiteralText(text) => text.render(py, template, context),
            Self::TranslatedText(text) => {
                TranslatedText::new(text.at).render(py, template, context)
            }
            Self::Int(n) => Ok(render_int(n, context)?.into()),
            Self::Float(f) => Content::Float(*f).render(context).map_err(Into::into),
            Self::Tag(tag) => tag.render(py, template, context),
            Self::Variable(variable) => {
                match resolve_output_variable(variable, py, template, context)? {
                    Some(value) => value.render(context).map_err(Into::into),
                    None => Ok(Cow::Borrowed("")),
                }
            }
            Self::ForVariable(variable) => variable.render(py, template, context),
            Self::Filter(filter) => filter.render(py, template, context),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use pyo3::types::{PyDict, PyList, PyString};

    #[test]
    fn test_render_variable() {
        Python::initialize();

        Python::attach(|py| {
            let name = PyString::new(py, "Lily").into_any();
            let context = HashMap::from([("name".to_string(), name.unbind())]);
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ name }}");
            let variable = Variable::new((3, 4));

            let rendered = variable.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "Lily");
        });
    }

    #[test]
    fn test_render_dict_lookup() {
        Python::initialize();

        Python::attach(|py| {
            let data = PyDict::new(py);
            let name = PyString::new(py, "Lily");
            data.set_item("name", name).unwrap();
            let context = HashMap::from([("data".to_string(), data.into_any().unbind())]);
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ data.name }}");
            let variable = Variable::new((3, 9));

            let rendered = variable.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "Lily");
        });
    }

    #[test]
    fn test_render_list_lookup() {
        Python::initialize();

        Python::attach(|py| {
            let name = PyString::new(py, "Lily");
            let names = PyList::new(py, [name]).unwrap();
            let context = HashMap::from([("names".to_string(), names.into_any().unbind())]);
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ names.0 }}");
            let variable = Variable::new((3, 7));

            let rendered = variable.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "Lily");
        });
    }

    #[test]
    fn test_render_attribute_lookup() {
        Python::initialize();

        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
class User:
    def __init__(self, name):
        self.name = name

user = User('Lily')
",
                None,
                Some(&locals),
            )
            .unwrap();

            let context: HashMap<String, Py<PyAny>> = locals.extract().unwrap();
            let mut context = Context::new(context, None, false);
            let template = TemplateString("{{ user.name }}");
            let variable = Variable::new((3, 9));

            let rendered = variable.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "Lily");
        });
    }

    #[test]
    fn test_render_html_autoescape() {
        Python::initialize();

        Python::attach(|py| {
            let html = PyString::new(py, "<p>Hello World!</p>").into_any().unbind();
            let context = HashMap::from([("html".to_string(), html)]);
            let mut context = Context::new(context, None, true);
            let template = TemplateString("{{ html }}");
            let html = Variable::new((3, 4));

            let rendered = html.render(py, template, &mut context).unwrap();
            assert_eq!(rendered, "&lt;p&gt;Hello World!&lt;/p&gt;");
        });
    }
}
