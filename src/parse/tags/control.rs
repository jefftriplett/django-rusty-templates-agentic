//! Control-flow tags: `if`, `for`, `with`, `cycle`, `autoescape`, and friends.
use crate::parse::Cycle;
use crate::parse::CycleId;
use crate::parse::CycleValue;
use crate::parse::EndTag;
use crate::parse::EndTagType;
use crate::parse::Filter;
use crate::parse::FirstOf;
use crate::parse::For;
use crate::parse::ForIterable;
use crate::parse::ForNames;
use crate::parse::ForParseError;
use crate::parse::NEXT_CYCLE_ID;
use crate::parse::NamedCycle;
use crate::parse::Parse;
use crate::parse::ParseError;
use crate::parse::Parser;
use crate::parse::PyParseError;
use crate::parse::SilentNamedCycle;
use crate::parse::SimpleCycle;
use crate::parse::Tag;
use crate::parse::TagElement;
use crate::parse::TokenTree;
use crate::parse::extract_as_variable;
use crate::parse::if_condition::parse_if_condition;
use crate::parse::parse_numeric;
use crate::types::Text;
use dtl_lexer::TemplateContent;
use dtl_lexer::common::text_content_at;
use dtl_lexer::common::translated_text_content_at;
use dtl_lexer::tag::TagParts;
use dtl_lexer::tag::autoescape::lex_autoescape_argument;
use dtl_lexer::tag::common::TagElementLexer;
use dtl_lexer::tag::common::TagElementTokenType;
use dtl_lexer::tag::forloop::ForLexer;
use dtl_lexer::tag::forloop::ForLexerInError;
use dtl_lexer::tag::forloop::ForTokenType;
use dtl_lexer::tag::kwarg::TagElementKwargLexer;
use dtl_lexer::types::At;
use dtl_lexer::variable::lex_variable_or_filter_with_syntax;
use num_bigint::BigInt;
use std::sync::atomic::Ordering;

fn parse_for_loop(
    parser: &mut Parser,
    parts: TagParts,
    at: At,
) -> Result<(ForIterable, ForNames, bool), ParseError> {
    let mut lexer = ForLexer::new(parser.template, parts)
        .with_modern_syntax(parser.engine.django_series >= (6, 0));
    let mut variable_names = Vec::new();
    while let Some(token) = lexer.lex_variable_name() {
        variable_names.push(token?);
    }
    if variable_names.is_empty() {
        return Err(ForParseError::MissingVariableNames { at: at.into() }.into());
    }
    let variables_start = variable_names[0].at.0;
    let last = variable_names
        .last()
        .expect("Variables has at least one element");
    let variables_at = (variables_start, last.at.0 - variables_start + last.at.1);

    if let Err(error) = lexer.lex_in() {
        if last.content(parser.template) != "in" {
            return Err(error.into());
        }
        let len = variable_names.len();
        match error {
            ForLexerInError::MissingComma { .. } if len >= 2 => {
                let previous = &variable_names[len - 2];
                let at = previous.at.into();
                return Err(ForParseError::MissingVariable { at }.into());
            }
            _ => {
                let at = last.at.into();
                return Err(ForParseError::MissingVariableBeforeIn { at }.into());
            }
        }
    }

    let expression_token = lexer.lex_expression()?;
    let reversed = lexer.lex_reversed()?;
    let variable_names = variable_names
        .iter()
        .map(|token| token.content(parser.template).to_string())
        .collect();
    let expression_content = expression_token.content(parser.template);
    let expression = match expression_token.token_type {
        ForTokenType::Numeric => parse_numeric(expression_content, expression_token.at)?,
        ForTokenType::Text => TagElement::Text(Text::new(text_content_at(expression_token.at))),
        ForTokenType::TranslatedText => {
            TagElement::TranslatedText(Text::new(translated_text_content_at(expression_token.at)))
        }
        ForTokenType::Variable => parser.parse_filter_expression(
            expression_content,
            expression_token.at,
            expression_token.at.0,
        )?,
    };
    Ok((
        ForIterable {
            iterable: expression,
            at: expression_token.at,
        },
        ForNames {
            names: variable_names,
            at: variables_at,
        },
        reversed,
    ))
}

impl Parser<'_, '_> {
    pub(in crate::parse) fn parse_resetcycle(
        &self,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let arguments: Vec<_> = self.template.content(parts.at).split_whitespace().collect();
        let error = |message: String| {
            crate::template::django_rusty_templates::TemplateSyntaxError::new_err(message)
        };
        let id = match arguments.as_slice() {
            [] => self
                .last_cycle
                .ok_or_else(|| error("No cycles in template.".to_string()))?,
            [name] => match self.named_cycles.get(*name) {
                Some(Cycle::Simple(cycle)) => cycle.id,
                Some(Cycle::Named(cycle)) => cycle.cycle.id,
                Some(Cycle::SilentNamed(cycle)) => cycle.cycle.cycle.id,
                None => return Err(error(format!("Named cycle '{name}' does not exist.")).into()),
            },
            _ => {
                return Err(
                    error("'resetcycle' tag accepts at most one argument.".to_string()).into(),
                );
            }
        };
        Ok(TokenTree::tag(Tag::ResetCycle(id)))
    }

    pub(in crate::parse) fn parse_ifchanged(
        &mut self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let values = TagElementKwargLexer::new(self.template, parts)
            .with_modern_syntax(self.engine.django_series >= (6, 0))
            .map(|token| token.map_err(ParseError::from)?.parse(self))
            .collect::<Result<Vec<_>, _>>()?;
        let end = EndTagType::Custom("endifchanged".to_string());
        let (truthy, stop) = self.parse_until(
            vec![
                EndTagType::Else,
                EndTagType::Custom("endifchanged".to_string()),
            ],
            "ifchanged".into(),
            at,
        )?;
        let falsey = if stop.end == EndTagType::Else {
            self.parse_until(vec![end], "else".into(), stop.at)?.0
        } else {
            Vec::new()
        };
        Ok(TokenTree::tag(Tag::IfChanged {
            id: CycleId(NEXT_CYCLE_ID.fetch_add(1, Ordering::Relaxed)),
            values,
            truthy,
            falsey,
        }))
    }

    pub(in crate::parse) fn parse_with(
        &mut self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let tokens = TagElementKwargLexer::new(self.template, parts)
            .with_modern_syntax(self.engine.django_series >= (6, 0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ParseError::from)?;
        let mut assignments = Vec::new();
        if !tokens.is_empty() && tokens.iter().all(|token| token.kwarg.is_some()) {
            for token in &tokens {
                let key = token.kwarg.expect("keyword was checked above");
                assignments.push((self.template.content(key).to_string(), token.parse(self)?));
            }
        } else {
            let mut remaining = tokens.as_slice();
            while let [value, as_token, name, rest @ ..] = remaining {
                if self.template.content(as_token.at) != "as" || name.kwarg.is_some() {
                    break;
                }
                assignments.push((
                    self.template.content(name.at).to_string(),
                    value.parse(self)?,
                ));
                remaining = rest;
                if let [and, rest @ ..] = remaining
                    && self.template.content(and.at) == "and"
                {
                    remaining = rest;
                } else {
                    break;
                }
            }
            if !remaining.is_empty() {
                assignments.clear();
            }
        }
        if assignments.is_empty() {
            return Err(
                crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                    "'with' expected at least one variable assignment",
                )
                .into(),
            );
        }
        let (nodes, _) = self.parse_until(
            vec![EndTagType::Custom("endwith".to_string())],
            "with".into(),
            at,
        )?;
        Ok(TokenTree::tag(Tag::With { assignments, nodes }))
    }

    pub(in crate::parse) fn parse_filter_block(
        &mut self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        // The synthetic numeric input aligns every real filter span with the source.
        let expression = format!("0|{}", self.template.content(parts.at));
        let (_, _, lexer) = lex_variable_or_filter_with_syntax(
            &expression,
            parts.at.0 - 2,
            self.engine.django_series >= (6, 0),
        )
        .map_err(ParseError::from)?
        .expect("the synthetic input is nonempty");
        let mut filters = Vec::new();
        for token in lexer {
            let token = token.map_err(ParseError::from)?;
            let name = self.template.content(token.at);
            if matches!(name, "safe" | "escape") {
                return Err(
                    crate::template::django_rusty_templates::TemplateSyntaxError::new_err(format!(
                        r#""{name}" filter is not permitted. Use the "autoescape" tag instead."#
                    ))
                    .into(),
                );
            }
            let right = token
                .argument
                .as_ref()
                .map(|argument| argument.parse(self))
                .transpose()?;
            filters.push(
                Filter::new(
                    self,
                    token.at,
                    token.at,
                    TagElement::Int(BigInt::ZERO),
                    right,
                )?
                .filter,
            );
        }
        let (nodes, _) = self.parse_until(
            vec![EndTagType::Custom("endfilter".to_string())],
            "filter".into(),
            at,
        )?;
        Ok(TokenTree::tag(Tag::FilterBlock { filters, nodes }))
    }

    pub(in crate::parse) fn parse_firstof(
        &self,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let mut tokens = TagElementKwargLexer::new(self.template, parts.clone())
            .with_modern_syntax(self.engine.django_series >= (6, 0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ParseError::from)?;

        let mut asvar = None;
        if tokens.len() >= 2 {
            let as_at = tokens[tokens.len() - 2].at;
            if self.template.content(as_at) == "as" {
                asvar = extract_as_variable(&mut tokens, &self.template)?;
            }
        }

        if tokens.is_empty() && asvar.is_none() {
            return Err(ParseError::MissingArgument {
                at: parts.at.into(),
            }
            .into());
        }

        let mut vars = Vec::with_capacity(tokens.len());
        for token in tokens {
            vars.push(token.parse(self)?);
        }

        Ok(TokenTree::tag(Tag::FirstOf(FirstOf { vars, asvar })))
    }

    pub(in crate::parse) fn parse_cycle(&mut self, parts: TagParts) -> Result<Cycle, ParseError> {
        let at = parts.at;
        let tokens = TagElementLexer::new(self.template, parts)
            .with_modern_syntax(self.engine.django_series >= (6, 0))
            .collect::<Result<Vec<_>, _>>()?;

        let tokens = match tokens.as_slice() {
            [] => return Err(ParseError::MissingCycleArguments { at: at.into() }),

            [reference] if reference.token_type == TagElementTokenType::Variable => {
                let name_text = self.template.content(reference.at);

                return self.named_cycles.get(name_text).cloned().ok_or_else(|| {
                    ParseError::UnknownNamedCycle {
                        name: name_text.to_string(),
                        at: reference.at.into(),
                    }
                });
            }

            [argument] => {
                return Err(ParseError::MissingCycleArguments {
                    at: argument.at.into(),
                });
            }

            tokens => tokens,
        };

        let (tokens, name_at, silent) = match tokens {
            [values @ .., as_token, name_token, flag_token]
                if values.len() >= 2 && self.template.content(as_token.at) == "as" =>
            {
                let flag = self.template.content(flag_token.at);

                if flag != "silent" {
                    return Err(ParseError::InvalidCycleFlag {
                        flag: flag.to_string(),
                        at: flag_token.at.into(),
                    });
                }

                (values, Some(name_token.at), true)
            }

            [values @ .., as_token, name_token]
                if values.len() >= 2 && self.template.content(as_token.at) == "as" =>
            {
                (values, Some(name_token.at), false)
            }

            values => (values, None, false),
        };

        let values = tokens
            .iter()
            .map(|token| {
                Ok(CycleValue {
                    value: token.parse(self)?,
                    at: token.at,
                })
            })
            .collect::<Result<Vec<_>, ParseError>>()?;

        let id = CycleId(NEXT_CYCLE_ID.fetch_add(1, Ordering::Relaxed));
        self.last_cycle = Some(id);
        let simple_cycle = SimpleCycle { id, values };

        let Some(name_at) = name_at else {
            return Ok(Cycle::Simple(simple_cycle));
        };
        let asvar = self.template.content(name_at).to_string();
        let named_cycle = NamedCycle {
            cycle: simple_cycle,
            asvar: asvar.clone(),
        };
        let cycle = if silent {
            Cycle::SilentNamed(SilentNamedCycle { cycle: named_cycle })
        } else {
            Cycle::Named(named_cycle)
        };

        self.named_cycles.insert(asvar, cycle.clone());

        Ok(cycle)
    }

    pub(in crate::parse) fn parse_autoescape(
        &mut self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let token = lex_autoescape_argument(self.template, parts).map_err(ParseError::from)?;
        let (nodes, _) = self.parse_until(vec![EndTagType::Autoescape], "autoescape".into(), at)?;
        Ok(TokenTree::tag(Tag::Autoescape {
            enabled: token.enabled,
            nodes,
        }))
    }

    pub(in crate::parse) fn parse_if(
        &mut self,
        at: At,
        parts: TagParts,
        start: &'static str,
    ) -> Result<TokenTree, PyParseError> {
        let condition = parse_if_condition(self, parts, at)?;
        let (nodes, end_tag) = self.parse_until(
            vec![EndTagType::Elif, EndTagType::Else, EndTagType::EndIf],
            start.into(),
            at,
        )?;
        let falsey = match end_tag {
            EndTag {
                at,
                end: EndTagType::Elif,
                parts: Some(parts),
            } => Some(vec![self.parse_if(at, parts, "elif")?]),
            EndTag {
                at,
                end: EndTagType::Else,
                parts: None,
            } => {
                let (nodes, _) = self.parse_until(vec![EndTagType::EndIf], "else".into(), at)?;
                Some(nodes)
            }
            EndTag {
                at: _end_at,
                end: EndTagType::EndIf,
                parts: None,
            } => None,
            _ => unreachable!(),
        };
        Ok(TokenTree::tag(Tag::If {
            condition,
            truthy: nodes,
            falsey,
        }))
    }

    pub(in crate::parse) fn parse_for(
        &mut self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        self.forloop_depth += 1;
        let (iterable, variables, reversed) = parse_for_loop(self, parts, at)?;
        let (nodes, end_tag) = self.parse_until(
            vec![EndTagType::Empty, EndTagType::EndFor],
            "for".into(),
            at,
        )?;
        self.forloop_depth -= 1;
        let empty = match end_tag {
            EndTag {
                at,
                end: EndTagType::Empty,
                parts: _parts,
            } => {
                let (nodes, _) = self.parse_until(vec![EndTagType::EndFor], "empty".into(), at)?;
                Some(nodes)
            }
            EndTag {
                at: _end_at,
                end: EndTagType::EndFor,
                parts: _parts,
            } => None,
            _ => unreachable!(),
        };
        let callback_free = callback_free(&nodes) && empty.as_deref().is_none_or(callback_free);
        Ok(TokenTree::tag(Tag::For(For {
            iterable,
            variables,
            reversed,
            body: nodes,
            empty,
            callback_free,
        })))
    }
}

/// Whether rendering `nodes` never hands a Django `Context` to Python.
///
/// Variables, filters and these built-in tags read the context only through
/// native lookups, and only scope it with push/pop pairs that end inside
/// them. Python can't observe their scopes, so they need no live layers.
fn callback_free(nodes: &[TokenTree]) -> bool {
    nodes.iter().all(|node| match node {
        TokenTree::Text(_)
        | TokenTree::LiteralText(_)
        | TokenTree::TranslatedText(_)
        | TokenTree::Int(_)
        | TokenTree::Float(_)
        | TokenTree::Variable(_)
        | TokenTree::ForVariable(_)
        | TokenTree::Filter(_) => true,
        TokenTree::Tag(tag) => match tag.as_ref() {
            Tag::If { truthy, falsey, .. } => {
                callback_free(truthy) && falsey.as_deref().is_none_or(callback_free)
            }
            Tag::For(for_tag) => for_tag.callback_free,
            Tag::With { nodes, .. }
            | Tag::Autoescape { nodes, .. }
            | Tag::FilterBlock { nodes, .. }
            | Tag::Spaceless(nodes)
            | Tag::Verbatim(nodes) => callback_free(nodes),
            Tag::Comment(_) | Tag::Load | Tag::TemplateTag(_) | Tag::Lorem(_) => true,
            _ => false,
        },
    })
}
