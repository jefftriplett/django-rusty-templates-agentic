//! Rust syntax for Django's stock tag libraries; runtime services stay in Django.

use std::borrow::Cow;

use dtl_lexer::tag::TagParts;
use dtl_lexer::types::At;
use pyo3::prelude::*;

use super::{EndTagType, Parser, PyParseError, Tag, TagElement, TokenTree};
use crate::template::django_rusty_templates::TemplateSyntaxError;

#[derive(Clone, Copy)]
pub(super) enum StockCompiler {
    Static,
    Prefix(&'static str),
    Flag(&'static str),
    Override(&'static str),
    Assign(&'static str),
    Info(&'static str),
    Cache,
    Translate,
    BlockTranslate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StockNode {
    Static {
        path: TagElement,
        target: Option<String>,
    },
    Prefix {
        setting: &'static str,
        target: Option<String>,
    },
    Flag {
        kind: &'static str,
        enabled: bool,
        nodes: Vec<TokenTree>,
    },
    Override {
        kind: &'static str,
        value: TagElement,
        nodes: Vec<TokenTree>,
    },
    Assign {
        kind: &'static str,
        value: Option<TagElement>,
        target: String,
    },
    Translate {
        root: TagElement,
        filters: Vec<crate::filters::FilterType>,
        noop: bool,
        message_context: Option<TagElement>,
        target: Option<String>,
    },
    BlockTranslate {
        singular: String,
        plural: Option<String>,
        variables: Vec<String>,
        bindings: Vec<(String, TagElement)>,
        counter: Option<(String, TagElement)>,
        message_context: Option<TagElement>,
        trimmed: bool,
        target: Option<String>,
        tag_name: String,
    },
    Cache {
        timeout: TagElement,
        name: String,
        vary: Vec<TagElement>,
        cache: Option<TagElement>,
        nodes: Vec<TokenTree>,
    },
}

impl StockNode {
    pub fn child_nodes(&self) -> Vec<&[TokenTree]> {
        match self {
            Self::Flag { nodes, .. } | Self::Override { nodes, .. } | Self::Cache { nodes, .. } => {
                vec![nodes]
            }
            _ => Vec::new(),
        }
    }
}

pub(super) fn compiler(py: Python<'_>, tag: &Bound<'_, PyAny>) -> PyResult<Option<StockCompiler>> {
    let Some(module) = tag
        .getattr("__module__")
        .ok()
        .and_then(|value| value.extract::<String>().ok())
    else {
        return Ok(None);
    };
    if !matches!(
        module.as_str(),
        "django.templatetags.static"
            | "django.templatetags.l10n"
            | "django.templatetags.tz"
            | "django.templatetags.i18n"
            | "django.templatetags.cache"
    ) {
        return Ok(None);
    }
    let Some(name) = tag
        .getattr("__name__")
        .ok()
        .and_then(|value| value.extract::<String>().ok())
    else {
        return Ok(None);
    };
    let kind = match (module.as_str(), name.as_str()) {
        ("django.templatetags.static", "do_static") => StockCompiler::Static,
        ("django.templatetags.static", "get_static_prefix") => StockCompiler::Prefix("STATIC_URL"),
        ("django.templatetags.static", "get_media_prefix") => StockCompiler::Prefix("MEDIA_URL"),
        ("django.templatetags.l10n", "localize_tag") => StockCompiler::Flag("localize"),
        ("django.templatetags.tz", "localtime_tag") => StockCompiler::Flag("localtime"),
        ("django.templatetags.tz", "timezone_tag") => StockCompiler::Override("timezone"),
        ("django.templatetags.i18n", "language") => StockCompiler::Override("language"),
        ("django.templatetags.tz", "get_current_timezone_tag") => StockCompiler::Assign("timezone"),
        ("django.templatetags.i18n", "do_get_available_languages") => {
            StockCompiler::Assign("available_languages")
        }
        ("django.templatetags.i18n", "do_get_current_language") => {
            StockCompiler::Assign("language")
        }
        ("django.templatetags.i18n", "do_get_current_language_bidi") => {
            StockCompiler::Assign("language_bidi")
        }
        ("django.templatetags.i18n", "do_get_language_info") => {
            StockCompiler::Info("language_info")
        }
        ("django.templatetags.i18n", "do_get_language_info_list") => {
            StockCompiler::Info("language_info_list")
        }
        ("django.templatetags.cache", "do_cache") => StockCompiler::Cache,
        ("django.templatetags.i18n", "do_translate") => StockCompiler::Translate,
        ("django.templatetags.i18n", "do_block_translate") => StockCompiler::BlockTranslate,
        _ => return Ok(None),
    };
    // Names alone must not reinterpret a replacement compiler's semantics.
    if !py.import(module.as_str())?.getattr(name.as_str())?.is(tag) {
        return Ok(None);
    }
    Ok(Some(kind))
}

impl Parser<'_, '_> {
    fn parse_block_translate(
        &mut self,
        tag: &str,
        args: Vec<At>,
    ) -> Result<StockNode, PyParseError> {
        use dtl_lexer::core::TokenType;
        let invalid = || TemplateSyntaxError::new_err(format!("Invalid arguments for '{tag}'"));
        let expression = |parser: &Self, span: At| {
            parser.parse_filter_expression(parser.template.content(span), span, span.0)
        };
        let mut index = 0;
        let mut seen = std::collections::HashSet::new();
        let mut bindings = Vec::new();
        let mut counter = None;
        let mut message_context = None;
        let mut trimmed = false;
        let mut target = None;
        while index < args.len() {
            let option = self.template.content(args[index]);
            index += 1;
            if !seen.insert(option) {
                return Err(invalid().into());
            }
            match option {
                "trimmed" => trimmed = true,
                "asvar" => {
                    target = Some(
                        self.template
                            .content(*args.get(index).ok_or_else(invalid)?)
                            .to_string(),
                    );
                    index += 1;
                }
                "context" => {
                    message_context =
                        Some(expression(self, *args.get(index).ok_or_else(invalid)?)?);
                    index += 1;
                }
                "with" | "count" => {
                    let mut values = Vec::new();
                    let modern = args.get(index).is_some_and(|span| {
                        self.template
                            .content(*span)
                            .split_once('=')
                            .is_some_and(|(name, _)| {
                                !name.is_empty()
                                    && name.chars().all(|character| {
                                        character.is_alphanumeric() || character == '_'
                                    })
                            })
                    });
                    while let Some(span) = args.get(index) {
                        let token = self.template.content(*span);
                        if modern {
                            let Some((name, _)) = token.split_once('=') else {
                                break;
                            };
                            if name.is_empty()
                                || !name.chars().all(|character| {
                                    character.is_alphanumeric() || character == '_'
                                })
                            {
                                break;
                            }
                            values.push((
                                name.to_string(),
                                expression(
                                    self,
                                    (span.0 + name.len() + 1, span.1 - name.len() - 1),
                                )?,
                            ));
                            index += 1;
                        } else {
                            if index + 2 >= args.len()
                                || self.template.content(args[index + 1]) != "as"
                            {
                                break;
                            }
                            values.push((
                                self.template.content(args[index + 2]).to_string(),
                                expression(self, *span)?,
                            ));
                            index += 3;
                            if args
                                .get(index)
                                .is_some_and(|span| self.template.content(*span) == "and")
                            {
                                index += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    let mut unique: Vec<(String, TagElement)> = Vec::new();
                    for (name, value) in values {
                        if let Some((_, previous)) =
                            unique.iter_mut().find(|(existing, _)| *existing == name)
                        {
                            *previous = value;
                        } else {
                            unique.push((name, value));
                        }
                    }
                    if unique.is_empty() || (option == "count" && unique.len() != 1) {
                        return Err(invalid().into());
                    }
                    if option == "count" {
                        counter = unique.pop();
                    } else {
                        bindings = unique;
                    }
                }
                _ => return Err(invalid().into()),
            }
        }
        let mut singular = String::new();
        let mut plural = None;
        let mut variables = Vec::new();
        let closing = format!("end{tag}");
        loop {
            let Some(token) = self.lexer.next() else {
                return Err(invalid().into());
            };
            let message = plural.as_mut().unwrap_or(&mut singular);
            match token.token_type {
                TokenType::Text => {
                    message.push_str(&token.content(self.template).replace('%', "%%"));
                }
                TokenType::Variable => {
                    let variable = token.content(self.template).trim();
                    message.push_str(&format!("%({variable})s"));
                    variables.push(variable.to_string());
                }
                _ => {
                    let content = token.content(self.template).trim();
                    if content == "plural" && counter.is_some() && plural.is_none() {
                        plural = Some(String::new());
                    } else if content == closing && (counter.is_none() || plural.is_some()) {
                        break;
                    } else {
                        return Err(invalid().into());
                    }
                }
            }
        }
        Ok(StockNode::BlockTranslate {
            singular,
            plural,
            variables,
            bindings,
            counter,
            message_context,
            trimmed,
            target,
            tag_name: tag.to_string(),
        })
    }

    pub(super) fn parse_stock(
        &mut self,
        at: At,
        tag: &str,
        compiler: StockCompiler,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let mut args = self.argument_spans(parts.clone());
        let invalid = || TemplateSyntaxError::new_err(format!("Invalid arguments for '{tag}'"));
        let expression = |parser: &Self, span: At| {
            parser.parse_filter_expression(parser.template.content(span), span, span.0)
        };
        let node = match compiler {
            StockCompiler::Static => {
                let Some(path) = args.first() else {
                    return Err(invalid().into());
                };
                let path = expression(self, *path)?;
                let target = if (args.len() == 1 && tag == "as")
                    || (args.len() >= 2 && self.template.content(args[args.len() - 2]) == "as")
                {
                    Some(
                        self.template
                            .content(*args.get(2).ok_or_else(|| {
                                pyo3::exceptions::PyIndexError::new_err("list index out of range")
                            })?)
                            .to_string(),
                    )
                } else {
                    None
                };
                StockNode::Static { path, target }
            }
            StockCompiler::Prefix(setting) => {
                let bits: Vec<_> = self.template.content(parts.at).split_whitespace().collect();
                let target = match bits.as_slice() {
                    [] => None,
                    ["as", rest @ ..] => Some(
                        rest.first()
                            .ok_or_else(|| {
                                pyo3::exceptions::PyIndexError::new_err("list index out of range")
                            })?
                            .to_string(),
                    ),
                    _ => return Err(invalid().into()),
                };
                StockNode::Prefix { setting, target }
            }
            StockCompiler::Flag(kind) => {
                let enabled = match args.as_slice() {
                    [] => true,
                    [value] if self.template.content(*value) == "on" => true,
                    [value] if self.template.content(*value) == "off" => false,
                    _ => return Err(invalid().into()),
                };
                let (nodes, _) = self.parse_until(
                    vec![EndTagType::Custom(format!("end{kind}"))],
                    Cow::Owned(tag.to_string()),
                    at,
                )?;
                StockNode::Flag {
                    kind,
                    enabled,
                    nodes,
                }
            }
            StockCompiler::Override(kind) => {
                let [value] = args.as_slice() else {
                    return Err(invalid().into());
                };
                let value = expression(self, *value)?;
                let (nodes, _) = self.parse_until(
                    vec![EndTagType::Custom(format!("end{kind}"))],
                    Cow::Owned(tag.to_string()),
                    at,
                )?;
                StockNode::Override { kind, value, nodes }
            }
            StockCompiler::Assign(kind) => {
                let bits: Vec<_> = self.template.content(parts.at).split_whitespace().collect();
                let ["as", target] = bits.as_slice() else {
                    return Err(invalid().into());
                };
                StockNode::Assign {
                    kind,
                    value: None,
                    target: (*target).to_string(),
                }
            }
            StockCompiler::Info(kind) => {
                let [start, value, keyword, target] = args.as_slice() else {
                    return Err(invalid().into());
                };
                if self.template.content(*start) != "for" || self.template.content(*keyword) != "as"
                {
                    return Err(invalid().into());
                }
                StockNode::Assign {
                    kind,
                    value: Some(expression(self, *value)?),
                    target: self.template.content(*target).to_string(),
                }
            }
            StockCompiler::Translate => {
                let Some(first) = args.first() else {
                    return Err(invalid().into());
                };
                let mut root = expression(self, *first)?;
                let mut filters = Vec::new();
                while let TagElement::Filter(filter) = root {
                    filters.push(filter.filter);
                    root = filter.left;
                }
                filters.reverse();
                let mut noop = false;
                let mut message_context = None;
                let mut target = None;
                let mut seen = std::collections::HashSet::new();
                let mut remaining = args[1..].iter();
                while let Some(option) = remaining.next() {
                    let option = self.template.content(*option);
                    if !seen.insert(option) {
                        return Err(invalid().into());
                    }
                    match option {
                        "noop" => noop = true,
                        "as" => {
                            target = Some(
                                self.template
                                    .content(*remaining.next().ok_or_else(invalid)?)
                                    .to_string(),
                            );
                        }
                        "context" => {
                            let span = *remaining.next().ok_or_else(invalid)?;
                            if matches!(self.template.content(span), "as" | "noop") {
                                return Err(invalid().into());
                            }
                            message_context = Some(expression(self, span)?);
                        }
                        _ => return Err(invalid().into()),
                    }
                }
                StockNode::Translate {
                    root,
                    filters,
                    noop,
                    message_context,
                    target,
                }
            }
            StockCompiler::BlockTranslate => self.parse_block_translate(tag, args)?,
            StockCompiler::Cache => {
                let (nodes, _) = self.parse_until(
                    vec![EndTagType::Custom("endcache".to_string())],
                    Cow::Owned(tag.to_string()),
                    at,
                )?;
                if args.len() < 2 {
                    return Err(invalid().into());
                }
                let cache = if args.len() > 2
                    && self
                        .template
                        .content(*args.last().expect("arguments exist"))
                        .starts_with("using=")
                {
                    let span = args.pop().expect("arguments exist");
                    Some(expression(self, (span.0 + 6, span.1 - 6))?)
                } else {
                    None
                };
                let timeout = expression(self, args[0])?;
                let name = self.template.content(args[1]).to_string();
                let vary = args[2..]
                    .iter()
                    .map(|span| expression(self, *span))
                    .collect::<Result<Vec<_>, _>>()?;
                StockNode::Cache {
                    timeout,
                    name,
                    vary,
                    cache,
                    nodes,
                }
            }
        };
        Ok(TokenTree::Tag(Tag::Stock {
            node: Box::new(node),
            at,
        }))
    }
}
