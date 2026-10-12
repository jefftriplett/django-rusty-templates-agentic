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

impl<T: Render> Render for Vec<T> {
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        self.as_slice().render(py, template, context)
    }
    fn render_into(
        &self,
        py: Python<'_>,
        template: TemplateString<'_>,
        context: &mut Context,
        output: &mut String,
    ) -> RenderIntoResult {
        self.as_slice().render_into(py, template, context, output)
    }
}

impl<T> Render for [T]
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
            1 => context.join_scope(|context| self[0].render(py, template, context)),
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
        // Django renders every sibling before joining their results. Defer a
        // non-string result until then, so a later callback error wins.
        context.join_scope(|context| {
            for (index, node) in self.iter().enumerate() {
                context.join_index = index;
                node.render_into(py, template, context, output)?;
            }
            Ok(())
        })
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
