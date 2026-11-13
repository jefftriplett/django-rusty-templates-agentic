//! Standalone built-in tags: `lorem`, `comment`, `now`, `regroup`, `widthratio`, and `url`.
use crate::gc::Gc;
use crate::parse::Comment;
use crate::parse::Lorem;
use crate::parse::Now;
use crate::parse::Parse;
use crate::parse::ParseError;
use crate::parse::Parser;
use crate::parse::PyParseError;
use crate::parse::Regroup;
use crate::parse::Tag;
use crate::parse::TagElement;
use crate::parse::TokenTree;
use crate::parse::Url;
use crate::parse::extract_as_variable;
use dtl_lexer::core::TokenType;
use dtl_lexer::tag::TagParts;
use dtl_lexer::tag::kwarg::TagElementKwargLexer;
use dtl_lexer::tag::lorem::LoremLexer;
use dtl_lexer::tag::lorem::LoremMethod;
use dtl_lexer::tag::lorem::LoremTokenType;
use dtl_lexer::tag::now::NowLexer;
use dtl_lexer::types::At;
use pyo3::prelude::*;

impl Parser<'_, '_> {
    pub(in crate::parse) fn parse_lorem(
        &self,
        _at: At,
        parts: TagParts,
    ) -> Result<Lorem, PyParseError> {
        let mut lexer = LoremLexer::new(self.template, parts);

        let first = match lexer.next() {
            None => {
                return Ok(Lorem {
                    count: TagElement::Int(1.into()),
                    method: LoremMethod::Blocks,
                    common: true,
                });
            }
            Some(first) => first.map_err(ParseError::from)?,
        };

        let second = match lexer.next() {
            None => match first.token_type {
                LoremTokenType::Method(method) => {
                    return Ok(Lorem {
                        count: TagElement::Int(1.into()),
                        method,
                        common: true,
                    });
                }
                LoremTokenType::Count(token) => {
                    let count = token.parse(self)?;
                    return Ok(Lorem {
                        count,
                        method: LoremMethod::Blocks,
                        common: true,
                    });
                }
                LoremTokenType::Random => {
                    return Ok(Lorem {
                        count: TagElement::Int(1.into()),
                        method: LoremMethod::Blocks,
                        common: false,
                    });
                }
            },
            Some(second) => second.map_err(ParseError::from)?,
        };

        let count =
            self.parse_filter_expression(self.template.content(first.at), first.at, first.at.0)?;
        let third = match lexer.next() {
            None => match second.token_type {
                LoremTokenType::Method(method) => {
                    return Ok(Lorem {
                        count,
                        method,
                        common: true,
                    });
                }
                LoremTokenType::Random => {
                    return Ok(Lorem {
                        count,
                        method: LoremMethod::Blocks,
                        common: false,
                    });
                }
                LoremTokenType::Count(_) => {
                    unreachable!("Count in second position should already have errored")
                }
            },
            Some(third) => third.map_err(ParseError::from)?,
        };

        if let Some(fourth) = lexer.next() {
            fourth.map_err(ParseError::from)?;
            unreachable!(
                "A fourth argument should be a duplicate count, method or random, which is already an error"
            )
        }

        match (second.token_type, third.token_type) {
            (LoremTokenType::Method(method), LoremTokenType::Random) => Ok(Lorem {
                count,
                method,
                common: false,
            }),
            _ => unreachable!("Should already have errored"),
        }
    }

    pub(in crate::parse) fn parse_comment(
        &mut self,
        at: At,
        _parts: TagParts,
    ) -> Result<Comment, PyParseError> {
        for token in self.lexer.by_ref() {
            if token.token_type == TokenType::Tag {
                // Like Django's `skip_past`, only an exact `endcomment` closes.
                if token.content(self.template).trim() == "endcomment" {
                    return Ok(Comment);
                }
            }
        }

        Err(ParseError::MissingEndTag {
            start: "comment".into(),
            expected: "endcomment".into(),
            at: at.into(),
        }
        .into())
    }

    pub(in crate::parse) fn parse_now(&self, parts: TagParts) -> Result<Now, PyParseError> {
        let version = self.py.import("django")?.getattr("VERSION")?;
        let series = (
            version.get_item(0)?.extract::<u8>()?,
            version.get_item(1)?.extract::<u8>()?,
        );
        if series >= (6, 2) {
            let mut tokens = self.argument_spans(parts);
            let asvar = if tokens.len() == 3 && self.template.content(tokens[1]) == "as" {
                let target = tokens.pop();
                tokens.pop();
                target
            } else {
                None
            };
            let [at] = tokens.as_slice() else {
                return Err(
                    crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                        "'now' statement takes one argument",
                    )
                    .into(),
                );
            };
            let expression = self.parse_filter_expression(self.template.content(*at), *at, at.0)?;
            let escape = !matches!(
                expression,
                TagElement::Text(_) | TagElement::TranslatedText(_)
            );
            return Ok(Now {
                format: String::new(),
                expression: Some(expression),
                escape,
                asvar,
            });
        }
        let mut lexer = NowLexer::new(self.template, parts);

        let format_at = lexer.lex_format().map_err(ParseError::from)?;
        let asvar = lexer.lex_variable().map_err(ParseError::from)?;
        lexer.extra_token().map_err(ParseError::from)?;
        let raw = self.template.content(format_at);
        // Django always trims the first and last character without further
        // validation, so so do we. This will become unnecessary if Django
        // starts supporting variables in the now tag.
        // https://github.com/django/new-features/issues/115
        let mut chars = raw.chars();
        chars.next();
        chars.next_back();
        let format = chars.as_str();

        Ok(Now {
            format: format.to_string(),
            expression: None,
            escape: false,
            asvar,
        })
    }

    pub(in crate::parse) fn parse_regroup(
        &self,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let tokens = TagElementKwargLexer::new(self.template, parts)
            .with_modern_syntax(self.engine.django_series >= (6, 0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ParseError::from)?;
        let error = crate::template::django_rusty_templates::TemplateSyntaxError::new_err;
        if tokens.len() != 5 {
            return Err(error("'regroup' tag takes five arguments").into());
        }
        if self.template.content(tokens[1].at) != "by" {
            return Err(error("second argument to 'regroup' tag must be 'by'").into());
        }
        if self.template.content(tokens[3].at) != "as" {
            return Err(error("next-to-last argument to 'regroup' tag must be 'as'").into());
        }
        let target = tokens[0].parse(self)?;
        let asvar = self.template.content(tokens[4].at).to_string();
        let key = self.template.content(tokens[2].at);
        let lookup = (!key.is_empty()
            && key.split('.').all(|part| {
                !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_')
            }))
        .then(|| key.split('.').map(str::to_string).collect());
        let expression = format!("{asvar}.{key}");
        let builtins = pyo3::types::PyList::empty(self.py);
        for module in [
            "django.template.defaulttags",
            "django.template.defaultfilters",
            "django.template.loader_tags",
        ] {
            builtins.append(self.py.import(module)?.getattr("register")?)?;
        }
        let parser = self
            .py
            .import("django.template.base")?
            .getattr("Parser")?
            .call1((
                pyo3::types::PyList::empty(self.py),
                self.py.None(),
                builtins,
            ))?;
        let filters = parser.getattr("filters")?;
        for (name, filter) in &self.external_filters {
            filters.set_item(name, filter)?;
        }
        let expression = Gc::new(
            parser
                .call_method1("compile_filter", (expression,))?
                .unbind(),
        );
        Ok(TokenTree::tag(Tag::Regroup(Regroup {
            target,
            expression,
            asvar,
            lookup,
        })))
    }

    pub(in crate::parse) fn parse_widthratio(
        &self,
        parts: TagParts,
    ) -> Result<TokenTree, PyParseError> {
        let mut tokens = TagElementKwargLexer::new(self.template, parts)
            .with_modern_syntax(self.engine.django_series >= (6, 0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ParseError::from)?;
        let asvar =
            if tokens.len() >= 2 && self.template.content(tokens[tokens.len() - 2].at) == "as" {
                extract_as_variable(&mut tokens, &self.template)?
            } else {
                None
            };
        if tokens.len() != 3 || tokens.iter().any(|token| token.kwarg.is_some()) {
            return Err(
                crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                    "widthratio takes three arguments",
                )
                .into(),
            );
        }
        let args = tokens
            .iter()
            .map(|token| token.parse(self))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TokenTree::tag(Tag::WidthRatio { args, asvar }))
    }

    pub(in crate::parse) fn parse_url(
        &self,
        at: At,
        parts: TagParts,
    ) -> Result<TokenTree, ParseError> {
        let mut lexer = TagElementKwargLexer::new(self.template, parts)
            .with_modern_syntax(self.engine.django_series >= (6, 0));
        let Some(view_token) = lexer.next() else {
            return Err(ParseError::UrlTagNoArguments { at: at.into() });
        };
        let view_name = view_token?.parse(self)?;

        let mut tokens = lexer.collect::<Result<Vec<_>, _>>()?;

        // We swallow errors here to match django's behavior at parsing time because we cannot
        // be sure if 'as' is supposed to be a variable or a regular 'as my_var' binding.
        let asvar = extract_as_variable(&mut tokens, &self.template).unwrap_or_default();

        let mut args = vec![];
        let mut kwargs = vec![];
        for token in tokens {
            let element = token.parse(self)?;
            match token.kwarg {
                None => args.push(element),
                Some(at) => {
                    let kwarg = self.template.content(at).to_string();
                    kwargs.push((kwarg, element));
                }
            }
        }
        let url = Url {
            at,
            view_name,
            args,
            kwargs,
            asvar,
        };
        Ok(TokenTree::tag(Tag::Url(url)))
    }
}
