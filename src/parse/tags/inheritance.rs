//! Template composition tags: `extends`, `block`, `include`, and `partialdef`.
use crate::gc::Gc;
use crate::parse::CycleId;
use crate::parse::EndTagType;
use crate::parse::Include;
use crate::parse::IncludeTemplateName;
use crate::parse::NEXT_CYCLE_ID;
use crate::parse::Parse;
use crate::parse::ParseError;
use crate::parse::Parser;
use crate::parse::PartialDefinition;
use crate::parse::PyParseError;
use crate::parse::RelativePath;
use crate::parse::Tag;
use crate::parse::TagElement;
use crate::parse::TokenTree;
use crate::parse::parse_include_template_token;
use crate::path::construct_relative_path;
use crate::types::Text;
use dtl_lexer::tag::TagParts;
use dtl_lexer::tag::include::IncludeLexer;
use dtl_lexer::tag::include::IncludeToken;
use dtl_lexer::tag::include::IncludeWithToken;
use dtl_lexer::types::At;
use pyo3::prelude::*;
use std::borrow::Cow;
use std::sync::atomic::Ordering;

impl Parser<'_, '_> {
    pub(in crate::parse) fn parse_partialdef(
        &mut self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        use crate::template::django_rusty_templates::TemplateSyntaxError;
        let tokens = self.argument_spans(parts);
        let (name, inline) = match tokens.as_slice() {
            [] => return Err(TemplateSyntaxError::new_err("'partialdef' tag requires a name").into()),
            [name] => (self.template.content(*name).to_string(), false),
            [name, inline] if self.template.content(*inline) == "inline" => (self.template.content(*name).to_string(), true),
            [_, _] => return Err(TemplateSyntaxError::new_err("The 'inline' argument does not have any parameters; either use 'inline' or remove it completely.").into()),
            _ => return Err(TemplateSyntaxError::new_err("'partialdef' tag takes at most 2 arguments").into()),
        };
        let (nodes, end) = self.parse_until(
            vec![EndTagType::Custom("endpartialdef".to_string())],
            Cow::Borrowed("partialdef"),
            at,
        )?;
        let close = self.end_tag_source(end.at);
        if close != "endpartialdef" && close != format!("endpartialdef {name}") {
            return Err(TemplateSyntaxError::new_err("Invalid endpartialdef name").into());
        }
        if !self.partial_names.insert(name.clone()) {
            return Err(TemplateSyntaxError::new_err(format!(
                "Partial '{name}' is already defined in the '{}' template.",
                self.warning_origin.as_deref().unwrap_or("<unknown source>")
            ))
            .into());
        }
        Ok(TokenTree::tag(Tag::PartialDef {
            definition: PartialDefinition {
                name,
                nodes: Gc::new(nodes),
                span: (at.0, end.at.0 + end.at.1 - at.0),
            },
            inline,
        }))
    }

    pub(in crate::parse) fn parse_block(
        &mut self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let names: Vec<_> = self.template.content(parts.at).split_whitespace().collect();
        let [name] = names.as_slice() else {
            return Err(
                crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                    "'block' tag takes only one argument",
                )
                .into(),
            );
        };
        let name = (*name).to_string();
        if !self.block_names.insert(name.clone()) {
            return Err(
                crate::template::django_rusty_templates::TemplateSyntaxError::new_err(format!(
                    "'block' tag with name '{name}' appears more than once"
                ))
                .into(),
            );
        }
        let (nodes, end) = self.parse_until(
            vec![EndTagType::Custom("endblock".to_string())],
            "block".into(),
            at,
        )?;
        if let Some(parts) = end.parts {
            let end_source = self.end_tag_source(end.at);
            let end_name = self.template.content(parts.at).trim();
            if end_source != "endblock" && end_source != format!("endblock {name}") {
                return Err(
                    crate::template::django_rusty_templates::TemplateSyntaxError::new_err(format!(
                        "Invalid endblock name '{end_name}' for block '{name}'"
                    ))
                    .into(),
                );
            }
        }
        Ok(TokenTree::tag(Tag::Block {
            name,
            nodes: Gc::new(nodes),
        }))
    }

    pub(in crate::parse) fn parse_extends(
        &mut self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let expression = self.template.content(parts.at);
        if expression.is_empty() {
            return Err(
                crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                    "'extends' takes one argument",
                )
                .into(),
            );
        }
        if self.has_extends {
            return Err(
                crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                    "'extends' cannot appear more than once in the same template",
                )
                .into(),
            );
        }
        self.has_extends = true;
        let parent = self.parse_filter_expression(expression, parts.at, parts.at.0)?;
        let parent_token = expression.to_string();
        let relative = if let TagElement::Text(text) = &parent {
            let name = self.template.content(text.at);
            if name.starts_with("./") || name.starts_with("../") {
                Some(
                    self.py
                        .import("django.template.loader_tags")?
                        .getattr("construct_relative_path")?
                        .call1((self.origin, name))?
                        .extract::<String>()?,
                )
            } else {
                None
            }
        } else {
            None
        };
        let nodes = self.parse_nodes()?;
        Ok(TokenTree::tag(Tag::Extends {
            at,
            parent,
            parent_token,
            relative,
            nodes,
        }))
    }

    pub(in crate::parse) fn parse_include(
        &self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, ParseError> {
        let mut lexer = IncludeLexer::new(self.template, parts)
            .with_modern_syntax(self.engine.django_series >= (6, 0));
        let Some(template_token) = lexer.lex_template()? else {
            return Err(ParseError::MissingArgument { at: at.into() });
        };
        let template_name = match parse_include_template_token(template_token, self)? {
            IncludeTemplateName::Text(Text { at }) => {
                let template_path = self.template.content(at);
                match construct_relative_path(template_path, self.origin, at)? {
                    Some(path) => IncludeTemplateName::Relative(RelativePath {
                        path: path.into_owned(),
                        at,
                    }),
                    None => IncludeTemplateName::Text(Text { at }),
                }
            }
            template_name => template_name,
        };

        let mut only = None;
        let mut with = None;
        let mut kwargs = Vec::new();
        match lexer.lex_with_or_only()? {
            IncludeWithToken::None => {}
            IncludeWithToken::Only(at) => {
                match lexer.lex_with_or_only()? {
                    IncludeWithToken::None => {}
                    IncludeWithToken::Only(second_at) => {
                        return Err(ParseError::IncludeOnlyTwice {
                            first_at: at.into(),
                            second_at: second_at.into(),
                        });
                    }
                    IncludeWithToken::With(at) => with = Some(at),
                }
                only = Some(at);
            }
            IncludeWithToken::With(at) => with = Some(at),
        }
        if let Some(with_at) = with {
            for token in lexer {
                match token? {
                    IncludeToken::Only(at) => match only {
                        None => only = Some(at),
                        Some(first_at) => {
                            return Err(ParseError::IncludeOnlyTwice {
                                first_at: first_at.into(),
                                second_at: at.into(),
                            });
                        }
                    },
                    IncludeToken::Kwarg { kwarg_at, token } => {
                        let element = token.parse(self)?;
                        kwargs.push((kwarg_at, element));
                    }
                }
            }
            if kwargs.is_empty() {
                return Err(ParseError::MissingKeywordArgument { at: with_at.into() });
            }
        }
        let include = Include {
            id: CycleId(NEXT_CYCLE_ID.fetch_add(1, Ordering::Relaxed)),
            template_name,
            origin: self.origin.map(ToString::to_string),
            engine: self.engine.clone(),
            kwargs,
            only: only.is_some(),
        };
        Ok(TokenTree::tag(Tag::Include(include)))
    }
}
