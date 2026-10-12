mod callbacks;
pub mod common;
pub mod filters;
pub mod inheritance;
pub mod lorem;
mod stock;
pub mod tags;
pub mod types;

use std::borrow::Cow;

use pyo3::prelude::*;

use dtl_lexer::types::TemplateString;

use crate::error::PyRenderError;
use types::{Content, Context};

pub type ResolveResult<'t, 'py> = Result<Option<Content<'t, 'py>>, PyRenderError>;
pub type RenderResult<'t> = Result<Cow<'t, str>, PyRenderError>;
pub type RenderIntoResult = Result<(), PyRenderError>;

pub(crate) fn join_type_error(index: usize, value: &Bound<'_, PyAny>) -> PyResult<PyErr> {
    let name = value.get_type().name()?.to_string();
    Ok(pyo3::exceptions::PyTypeError::new_err(format!(
        "sequence item {index}: expected str instance, {name} found"
    )))
}

/// Retain non-string results until joining, just as Django's NodeList does.
/// Valid string rendering does not allocate a result list.
#[derive(Default)]
pub(crate) struct JoinResults(Vec<(usize, Py<PyAny>)>);

impl JoinResults {
    pub(crate) fn push(&mut self, index: usize, value: Py<PyAny>) {
        self.0.push((index, value));
    }
    pub(crate) fn collect(&mut self, index: usize, result: RenderIntoResult) -> RenderIntoResult {
        match result {
            Err(PyRenderError::InvalidTagResult(value)) => {
                self.push(index, value);
                Ok(())
            }
            result => result,
        }
    }
    pub(crate) fn finish(self, py: Python<'_>) -> RenderIntoResult {
        if let Some((index, value)) = self.0.first() {
            return Err(join_type_error(*index, value.bind(py))?.into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveFailures {
    Raise,
    IgnoreVariableDoesNotExist,
}

/// Trait for resolving a template element into content suitable for
/// further processing by another template element.
trait Resolve {
    fn resolve<'t, 'py>(
        &self,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
        failures: ResolveFailures,
    ) -> ResolveResult<'t, 'py>;
}

/// Trait for rendering a template element into content suitable for
/// output in the completely processed template.
pub trait Render {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t>;

    /// Append output while keeping Python-owned text alive only for the copy.
    /// Capture boundaries can retain the default owned-result implementation.
    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> RenderIntoResult {
        output.push_str(&self.render(py, template, context)?);
        Ok(())
    }
}

/// Trait for evaluating an expression in a boolean context
pub trait Evaluate {
    fn evaluate(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
    ) -> Option<bool>;
}

impl<T> Evaluate for Option<T>
where
    T: Evaluate,
{
    fn evaluate(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
    ) -> Option<bool> {
        match self {
            Some(inner) => inner.evaluate(py, template, context),
            None => Some(false),
        }
    }
}

/// All resolvable template elements can be rendered
impl<T> Render for T
where
    T: Resolve,
{
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        match self.resolve(py, template, context, ResolveFailures::Raise)? {
            Some(content) => Ok(content.render(context)?),
            None => Ok(Cow::Borrowed("")),
        }
    }

    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> RenderIntoResult {
        if let Some(content) = self.resolve(py, template, context, ResolveFailures::Raise)? {
            content.render_into(context, output)?;
        }
        Ok(())
    }
}

impl<T> Render for Vec<T>
where
    T: Render,
{
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        match self.len() {
            0 => Ok(Cow::Borrowed("")),
            1 => match self[0].render(py, template, context) {
                Err(PyRenderError::InvalidTagResult(value)) => {
                    let mut join = JoinResults::default();
                    join.push(0, value);
                    join.finish(py)?;
                    Ok(Cow::Borrowed(""))
                }
                result => result,
            },
            _ => {
                let mut rendered = String::new();
                self.render_into(py, template, context, &mut rendered)?;
                Ok(Cow::Owned(rendered))
            }
        }
    }
    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> RenderIntoResult {
        let mut join = JoinResults::default();
        for (index, node) in self.iter().enumerate() {
            join.collect(index, node.render_into(py, template, context, output))?;
        }
        join.finish(py)
    }
}

impl<T> Render for Option<T>
where
    T: Render,
{
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        Ok(match self {
            Some(inner) => inner.render(py, template, context)?,
            None => Cow::Borrowed(""),
        })
    }
}
