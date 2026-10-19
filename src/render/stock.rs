use std::borrow::Cow;

use super::filters::ResolveFilter;
use dtl_lexer::types::TemplateString;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyDict, PyList, PyModule, PyString};

use super::types::{Content, Context};
use super::{Render, RenderResult, Resolve, ResolveFailures};
use crate::parse::{StockNode, TagElement};

static SERVICES: PyOnceLock<Py<PyModule>> = PyOnceLock::new();

fn services(py: Python<'_>) -> PyResult<&Bound<'_, PyModule>> {
    Ok(SERVICES
        .get_or_try_init(py, || {
            py.import("django_rusty_templates.stock").map(Bound::unbind)
        })?
        .bind(py))
}

fn value<'py>(
    expression: &TagElement,
    py: Python<'py>,
    template: TemplateString<'_>,
    context: &mut Context,
) -> Result<Bound<'py, PyAny>, crate::error::PyRenderError> {
    match expression.resolve(py, template, context, ResolveFailures::Raise)? {
        Some(content) => Ok(content.to_py(py)),
        None => Ok(context.invalid_value(py, None)?),
    }
}

impl Render for StockNode {
    #[expect(clippy::too_many_lines, reason = "one match arm per stock node")]
    fn render<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> RenderResult<'t> {
        let service = services(py)?;
        Ok(match self {
            Self::Static { path, target } => {
                let path = value(path, py, template, context)?;
                let output = service
                    .getattr("static_url")?
                    .call1((path, context.autoescape))?;
                if let Some(target) = target {
                    context.insert(target.clone(), output)?;
                    Cow::Borrowed("")
                } else {
                    Cow::Owned(output.extract()?)
                }
            }
            Self::Prefix { setting, target } => {
                let output = service.getattr("prefix")?.call1((setting,))?;
                if let Some(target) = target {
                    context.insert(target.clone(), output)?;
                    Cow::Borrowed("")
                } else {
                    Cow::Owned(output.extract()?)
                }
            }
            Self::Assign {
                kind,
                value: expression,
                target,
            } => {
                let input = expression
                    .as_ref()
                    .map(|expression| value(expression, py, template, context))
                    .transpose()?;
                let output = service.getattr("assigned_value")?.call1((kind, input))?;
                context.insert(target.clone(), output)?;
                Cow::Borrowed("")
            }
            Self::Flag {
                kind,
                enabled,
                nodes,
            } => {
                let localize = *kind == "localize";
                let previous = if localize {
                    context.use_l10n.replace(*enabled)
                } else {
                    context.use_tz.replace(*enabled)
                };
                let live = context
                    .formatting_context
                    .as_ref()
                    .map(|context| context.bind(py).clone());
                let attribute = if localize { "use_l10n" } else { "use_tz" };
                if let Some(live) = &live {
                    live.setattr(attribute, enabled)?;
                }
                let output = nodes.render(py, template, context);
                if localize {
                    context.use_l10n = previous;
                } else {
                    context.use_tz = previous;
                }
                if let Some(live) = live {
                    live.setattr(attribute, previous)?;
                }
                output?
            }
            Self::Override {
                kind,
                value: expression,
                nodes,
            } => {
                let input = value(expression, py, template, context)?;
                let scope = service.getattr("override")?.call1((kind, input))?;
                scope.call_method0("__enter__")?;
                // The cached language or timezone is stale inside the override.
                let date_cache = std::mem::take(&mut context.date_cache);
                let previous_format = std::mem::take(&mut context.number_format);
                let output = nodes.render(py, template, context);
                context.number_format = previous_format;
                context.date_cache = date_cache;
                scope.call_method1("__exit__", (py.None(), py.None(), py.None()))?;
                output?
            }
            Self::Translate {
                root,
                filters,
                noop,
                message_context,
                target,
            } => {
                let message_context = message_context
                    .as_ref()
                    .map(|expression| value(expression, py, template, context))
                    .transpose()?;
                // Django compiles _("literal") into a translation promise.
                // TranslateNode's noop/context options do not retranslate it.
                let literal = match root {
                    TagElement::TranslatedText(text) => Some(TagElement::Text(*text)),
                    _ => None,
                };
                let resolved = literal.as_ref().unwrap_or(root).resolve(
                    py,
                    template,
                    context,
                    ResolveFailures::IgnoreVariableDoesNotExist,
                )?;
                let missing = resolved.is_none();
                let safe = if matches!(root, TagElement::Text(_) | TagElement::TranslatedText(_)) {
                    Some(true)
                } else {
                    None
                };
                let input = match resolved {
                    Some(Content::String(text)) if safe.is_some() => {
                        PyString::new(py, text.as_raw()).into_any()
                    }
                    Some(content) => content.to_py(py),
                    None => {
                        let name = match root {
                            TagElement::Variable(variable) => Some(template.content(variable.at)),
                            _ => None,
                        };
                        context.invalid_value(py, name)?
                    }
                };
                let noop = missing || (literal.is_none() && *noop);
                let message_context = if literal.is_some() {
                    None
                } else {
                    message_context
                };
                let output = if filters.is_empty() {
                    service.getattr("render_translation")?.call1((
                        input,
                        noop,
                        message_context,
                        context.autoescape,
                        context.use_l10n,
                        context.use_tz,
                        context
                            .formatting_context
                            .as_ref()
                            .map(|live| live.bind(py)),
                        safe,
                    ))?
                } else {
                    let initial = service.getattr("translate")?.call1((
                        input,
                        noop,
                        message_context,
                        false,
                        safe,
                    ))?;
                    let mut result = Some(Content::Py(initial));
                    if !missing || context.string_if_invalid.is_empty() {
                        for filter in filters {
                            result = filter.resolve(result, py, template, context)?;
                        }
                    }
                    let result = result
                        .map_or_else(|| py.None().into_bound(py), |content| content.to_py(py));
                    service
                        .getattr("finish_translation")?
                        .call1((result, context.value_formatting_context(py)?))?
                };
                if let Some(target) = target {
                    context.insert(target.clone(), output)?;
                    Cow::Borrowed("")
                } else {
                    Cow::Owned(output.extract()?)
                }
            }
            Self::BlockTranslate { .. } => {
                return self.render_block_translation(py, template, context, false);
            }
            Self::Cache {
                timeout,
                name,
                vary,
                cache,
                nodes,
            } => {
                let timeout = value(timeout, py, template, context)?;
                let vary = PyList::new(
                    py,
                    vary.iter()
                        .map(|expression| value(expression, py, template, context))
                        .collect::<Result<Vec<_>, _>>()?,
                )?;
                let explicit_cache = cache.is_some();
                let cache = cache
                    .as_ref()
                    .map(|expression| value(expression, py, template, context))
                    .transpose()?;
                let entry = service.getattr("cache_entry")?.call1((
                    timeout,
                    name,
                    vary,
                    cache,
                    explicit_cache,
                ))?;
                let cached = entry.get_item(0)?;
                if cached.is_none() {
                    let output = nodes.render(py, template, context)?;
                    entry.get_item(1)?.call_method1(
                        "set",
                        (entry.get_item(2)?, output.as_ref(), entry.get_item(3)?),
                    )?;
                    output
                } else {
                    Cow::Owned(Content::Py(cached).to_py(py).extract()?)
                }
            }
        })
    }
}

impl StockNode {
    #[expect(
        clippy::too_many_lines,
        reason = "follows Django's BlockTranslateNode.render step by step"
    )]
    fn render_block_translation<'t>(
        &self,
        py: Python<'_>,
        template: TemplateString<'t>,
        context: &mut Context,
        nested: bool,
    ) -> RenderResult<'t> {
        let Self::BlockTranslate {
            singular,
            plural,
            variables,
            bindings,
            counter,
            message_context,
            trimmed,
            target,
            tag_name,
        } = self
        else {
            unreachable!("block translation renderer");
        };
        let service = services(py)?;
        let message_context = message_context
            .as_ref()
            .map(|expression| value(expression, py, template, context))
            .transpose()?;
        let resolved = bindings
            .iter()
            .map(|(name, expression)| {
                value(expression, py, template, context).map(|value| (name, value))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let scope = context
            .formatting_context
            .as_ref()
            .map(|live| live.bind(py).call_method0("push"))
            .transpose()?;
        let mut names = Vec::new();
        for (name, value) in resolved {
            if let Some(live) = &context.formatting_context {
                live.bind(py).set_item(name, &value)?;
            }
            context.append(name.clone(), value);
            names.push(name.clone());
        }
        let result = (|| {
            let count = if let Some((name, expression)) = counter
                .as_ref()
                .filter(|_| plural.as_ref().is_some_and(|message| !message.is_empty()))
            {
                let count = value(expression, py, template, context)?;
                if let Some(live) = &context.formatting_context {
                    live.bind(py).set_item(name, &count)?;
                }
                context.append(name.clone(), count.clone());
                names.push(name.clone());
                Some(count)
            } else {
                None
            };
            let message = service.getattr("block_translation_message")?.call1((
                singular,
                plural,
                count,
                counter.as_ref().map(|(name, _)| name),
                message_context,
                trimmed,
                tag_name,
            ))?;
            let data = PyDict::new(py);
            for name in variables {
                let input = context
                    .resolve_root(py, name)?
                    .unwrap_or(context.invalid_value(py, Some(name))?);
                let output = py
                    .import("django.template.base")?
                    .getattr("render_value_in_context")?
                    .call1((input, context.value_formatting_context(py)?))?;
                data.set_item(name, output)?;
            }
            let result = service
                .getattr("finish_block_translation")?
                .call1((message, data, tag_name, nested))?;
            Ok::<_, crate::error::PyRenderError>(result)
        })();
        for name in names.iter().rev() {
            context.pop_variable(name);
        }
        if let Some(scope) = scope {
            scope.call_method1("__exit__", (py.None(), py.None(), py.None()))?;
        }
        let output = result?;
        if output.is_none() {
            let override_scope = service
                .getattr("override")?
                .call1(("language", py.None()))?;
            override_scope.call_method0("__enter__")?;
            let date_cache = std::mem::take(&mut context.date_cache);
            let previous_format = std::mem::take(&mut context.number_format);
            let output = self.render_block_translation(py, template, context, true);
            context.number_format = previous_format;
            context.date_cache = date_cache;
            override_scope.call_method1("__exit__", (py.None(), py.None(), py.None()))?;
            return output;
        }
        if let Some(target) = target {
            let safe = py
                .import("django.utils.safestring")?
                .getattr("mark_safe")?
                .call1((output,))?;
            context.insert(target.clone(), safe)?;
            Ok(Cow::Borrowed(""))
        } else {
            Ok(Cow::Owned(output.extract()?))
        }
    }
}
