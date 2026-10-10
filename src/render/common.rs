use std::borrow::Cow;

use pyo3::exceptions::{PyAttributeError, PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyString;

use dtl_lexer::types::TemplateString;

use super::types::{AsBorrowedContent, Content, ContentString, Context};
use super::{Evaluate, Render, RenderResult, Resolve, ResolveFailures, ResolveResult};
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
static SIGNATURE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

static GETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Helper function to translate a string using Django's gettext
pub fn gettext(py: Python<'_>, text: &str) -> PyResult<String> {
    let get_text = GETTEXT.import(py, "django.utils.translation", "gettext")?;
    get_text.call1((text,))?.extract::<String>()
}

fn has_truthy_attr(variable: &Bound<'_, PyAny>, attr: &Bound<'_, PyString>) -> PyResult<bool> {
    match variable.getattr(attr) {
        Ok(attr) => attr.is_truthy(),
        Err(error) if error.is_instance_of::<PyAttributeError>(variable.py()) => Ok(false),
        Err(error) => Err(error),
    }
}

fn resolve_callable(variable: Bound<'_, PyAny>) -> PyResult<Option<Bound<'_, PyAny>>> {
    if !variable.is_callable() {
        return Ok(Some(variable));
    }
    let py = variable.py();
    if has_truthy_attr(&variable, intern!(py, "do_not_call_in_templates"))? {
        return Ok(Some(variable));
    }
    if has_truthy_attr(&variable, intern!(py, "alters_data"))? {
        return Ok(None);
    }
    match variable.call0() {
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

fn lookup_part<'py>(
    variable: Bound<'py, PyAny>,
    part: &str,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let py = variable.py();
    if variable.get_type().hasattr("__getitem__")? {
        match variable.get_item(part) {
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
    let base_context = BASE_CONTEXT.import(py, "django.template.context", "BaseContext")?;
    let is_context = variable.is_instance(base_context)?;
    if !is_context || !variable.get_type().hasattr(part)? {
        match variable.getattr(part) {
            Ok(value) => return Ok(Some(value)),
            Err(error)
                if error.is_instance_of::<PyTypeError>(py)
                    || error.is_instance_of::<PyAttributeError>(py) =>
            {
                if !is_context
                    && DIR
                        .import(py, "builtins", "dir")?
                        .call1((&variable,))?
                        .contains(part)?
                {
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
        let mut parts = self.parts(template);
        let (first, mut object_at) = parts.next().expect("Variable names cannot be empty");
        let Some(variable) = context.resolve_root(py, first)? else {
            return Ok(None);
        };
        let mut variable = match resolve_callable(variable) {
            Ok(Some(variable)) => variable,
            Ok(None) => return Ok(Some(Content::Py(context.invalid_value(py, None)?))),
            Err(error) if silent_failure(&error, py) => {
                return Ok(Some(Content::Py(context.invalid_value(py, None)?)));
            }
            Err(error) => return Err(error.annotate(py, self.at, "here", template).into()),
        };
        for (part, key_at) in parts {
            let resolved = lookup_part(variable.clone(), part).and_then(|value| match value {
                Some(value) => resolve_callable(value),
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
        context: &mut Context,
        _failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        let resolved = unescape_literal(template.content(self.at));
        Ok(Some(Content::String(match context.autoescape {
            false => ContentString::String(resolved),
            true => ContentString::HtmlSafe(resolved),
        })))
    }
}

impl Resolve for TranslatedText {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
        _failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py> {
        let text = template.content(self.at);
        let resolved = gettext(py, &unescape_literal(text))?;
        Ok(Some(Content::String(match context.autoescape {
            false => ContentString::String(Cow::Owned(resolved)),
            true => ContentString::HtmlSafe(Cow::Owned(resolved)),
        })))
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
                return variable.resolve(py, template, context, failures);
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

impl Evaluate for TagElement {
    fn evaluate(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
    ) -> Option<bool> {
        self.resolve(
            py,
            template,
            context,
            ResolveFailures::IgnoreVariableDoesNotExist,
        )
        .ok()?
        .evaluate(py, template, context)
    }
}

impl Render for TokenTree {
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
            Self::Int(n) => Ok(n.to_string().into()),
            Self::Float(f) => Ok(f.to_string().into()),
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

            let context = locals.extract().unwrap();
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
