use crate::gc::Gc;
mod builtin_filter;
mod errors;
mod filter;
mod if_condition;
mod nodes;
mod stock;
mod tags;
pub use errors::{Fallback, ForParseError, ParseError, PyParseError, Unimplemented};
pub use filter::Filter;
pub use if_condition::IfCondition;
pub use nodes::{
    Comment, CsrfToken, Cycle, CycleId, CycleValue, FirstOf, For, ForIterable, ForNames, Include,
    IncludeTemplateName, InclusionTag, Lorem, NamedCycle, Now, PartialDefinition, PythonNode,
    Regroup, RelativePath, SilentNamedCycle, SimpleBlockTag, SimpleCycle, SimpleTag, Tag,
    TagElement, TokenTree, Url,
};
pub use stock::StockNode;
use tags::TagContext;

use builtin_filter::BuiltinFilter;

use dtl_lexer::DelimitedToken;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use std::sync::atomic::{AtomicUsize, Ordering};

use either::Either;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;

use dtl_lexer::common::{
    Number, get_all_at, python_number, text_content_at, translated_text_content_at,
};
use dtl_lexer::core::{Lexer, TokenType};
use dtl_lexer::tag::common::{TagElementToken, TagElementTokenType};
use dtl_lexer::tag::include::{IncludeTemplateToken, IncludeTemplateTokenType};
use dtl_lexer::tag::kwarg::TagElementKwargToken;
use dtl_lexer::tag::templatetag::lex_templatetag;
use dtl_lexer::tag::{TagParts, lex_tag};
use dtl_lexer::types::{At, TemplateString};
use dtl_lexer::variable::{
    Argument as ArgumentToken, FilterLexer, VariableToken, lex_variable_or_filter_with_syntax,
};
use dtl_lexer::{START_TAG_LEN, TemplateContent};

use crate::template::django_rusty_templates::Engine;
use crate::types::Argument;
use crate::types::ArgumentType;
use crate::types::ForVariable;
use crate::types::ForVariableName;

use crate::types::Text;
use crate::types::TranslatedText;
use dtl_lexer::types::Variable;

// Assign each compiled cycle node a unique ID so its current position can be
// stored per render in Context without interfering with other cycle nodes.
static NEXT_CYCLE_ID: AtomicUsize = AtomicUsize::new(0);

trait Parse<R> {
    fn parse(&self, parser: &Parser) -> Result<R, ParseError>;
}

impl Parse<Argument> for ArgumentToken {
    fn parse(&self, parser: &Parser) -> Result<Argument, ParseError> {
        Ok(match *self {
            Self::Variable(at) => {
                let content = parser.template.content(at);
                // A number written with another script's digits, such as `١٢`.
                let number = if content.is_ascii() {
                    None
                } else {
                    python_number(content)
                };
                let argument_type = match number {
                    Some(Number::Int(n)) => ArgumentType::Int(n),
                    Some(Number::Float(f)) => ArgumentType::Float(f),
                    None => parser.parse_variable_reference(at).into(),
                };
                Argument { at, argument_type }
            }
            Self::Text(at) => Argument {
                at,
                argument_type: ArgumentType::Text(Text::new(self.content_at())),
            },
            Self::TranslatedText(at) => Argument {
                at,
                argument_type: ArgumentType::TranslatedText(TranslatedText::new(self.content_at())),
            },
            Self::Numeric(at) => {
                let content = parser.template.content(at);
                let argument_type = match python_number(content) {
                    Some(Number::Int(n)) => ArgumentType::Int(n),
                    Some(Number::Float(f)) => ArgumentType::Float(f),
                    // Not a number after all, so Django looks it up as a
                    // variable, as with `2.` or `1__0`.
                    None if content.starts_with(['-', '+']) => {
                        return Err(ParseError::InvalidNumber { at: at.into() });
                    }
                    None => parser.parse_variable_reference(at).into(),
                };
                Argument { at, argument_type }
            }
        })
    }
}

fn parse_numeric(content: &str, at: At) -> Result<TagElement, ParseError> {
    match python_number(content) {
        Some(Number::Int(n)) => Ok(TagElement::Int(n)),
        Some(Number::Float(f)) => Ok(TagElement::Float(f)),
        None => Err(ParseError::InvalidNumber { at: at.into() }),
    }
}

impl Parse<TagElement> for TagElementToken {
    fn parse(&self, parser: &Parser) -> Result<TagElement, ParseError> {
        let content_at = self.content_at();
        let (start, _len) = content_at;
        let content = parser.template.content(content_at);
        match self.token_type {
            TagElementTokenType::Numeric => parse_numeric(content, self.at),
            TagElementTokenType::Text => Ok(TagElement::Text(Text::new(content_at))),
            TagElementTokenType::TranslatedText => {
                Ok(TagElement::TranslatedText(Text::new(content_at)))
            }
            TagElementTokenType::Variable => {
                parser.parse_filter_expression(content, content_at, start)
            }
        }
    }
}

impl Parse<TagElement> for TagElementKwargToken {
    fn parse(&self, parser: &Parser) -> Result<TagElement, ParseError> {
        let content_at = self.content_at();
        let (start, _len) = content_at;
        let content = parser.template.content(content_at);
        match self.token_type {
            TagElementTokenType::Numeric => parse_numeric(content, self.at),
            TagElementTokenType::Text => Ok(TagElement::Text(Text::new(content_at))),
            TagElementTokenType::TranslatedText => {
                Ok(TagElement::TranslatedText(Text::new(content_at)))
            }
            TagElementTokenType::Variable => {
                parser.parse_filter_expression(content, content_at, start)
            }
        }
    }
}

/// Extracts "as variable" from the end of the tokens list (and truncates it).
///
/// This will return:
/// - `Ok(Some(variable_name))` if found and valid
/// - Ok(None) if "as" not found
/// - `ParseError` if as is not at the end or missing a variable
fn extract_as_variable(
    tokens: &mut Vec<TagElementKwargToken>,
    template: &TemplateString<'_>,
) -> Result<Option<String>, ParseError> {
    let len = tokens.len();
    if len < 1 {
        return Ok(None);
    }
    for (idx, token) in tokens.iter().rev().enumerate() {
        if template.content(token.at) == "as" {
            return match idx {
                0 => {
                    // Last token is "as". Check if the previous token is also "as",
                    // making this a valid "as <variable>" binding where variable is "as".
                    if len >= 2 && template.content(tokens[len - 2].at) == "as" {
                        tokens.truncate(len - 2);
                        Ok(Some("as".to_string()))
                    } else {
                        Err(ParseError::MissingVariableAfterAs {
                            at: token.at.into(),
                        })
                    }
                }
                1 => {
                    let variable = template.content(tokens[len - 1].at).to_string();
                    tokens.truncate(len - 2);
                    Ok(Some(variable))
                }
                _ => {
                    let asvar = tokens[len - idx].at;
                    let next = tokens[len - idx + 1].at;
                    let last = tokens[len - 1].at;
                    Err(ParseError::UnexpectedTokensAfterAsVariable {
                        at: get_all_at(next, last).into(),
                        var_name: template.content(asvar).to_string(),
                    })
                }
            };
        }
    }
    Ok(None)
}

fn parse_include_template_token(
    token: IncludeTemplateToken,
    parser: &Parser,
) -> Result<IncludeTemplateName, ParseError> {
    let content_at = token.content_at();
    let (start, _len) = content_at;
    let content = parser.template.content(content_at);
    Ok(match token.token_type {
        IncludeTemplateTokenType::Text => IncludeTemplateName::Text(Text::new(content_at)),
        IncludeTemplateTokenType::Variable => IncludeTemplateName::Variable(
            parser.parse_filter_expression(content, content_at, start)?,
        ),
    })
}

#[derive(PartialEq, Eq)]
enum EndTagType {
    Autoescape,
    Elif,
    Else,
    EndIf,
    Empty,
    EndFor,
    Verbatim,
    Custom(String),
}

impl EndTagType {
    fn as_cow(&self) -> Cow<'static, str> {
        let end_tag = match self {
            Self::Autoescape => "endautoescape",
            Self::Elif => "elif",
            Self::Else => "else",
            Self::EndIf => "endif",
            Self::Empty => "empty",
            Self::EndFor => "endfor",
            Self::Verbatim => "endverbatim",
            Self::Custom(s) => return Cow::Owned(s.clone()),
        };
        Cow::Borrowed(end_tag)
    }
}

#[derive(PartialEq, Eq)]
struct EndTag {
    at: At,
    end: EndTagType,
    parts: Option<TagParts>,
}

impl EndTag {
    fn as_cow(&self) -> Cow<'static, str> {
        self.end.as_cow()
    }
}

type BuiltinFilterCache = [Option<Gc<Py<PyAny>>>; BuiltinFilter::COUNT];

pub struct Parser<'t, 'py> {
    py: Python<'py>,
    template: TemplateString<'t>,
    lexer: Lexer<'t>,
    engine: Gc<Engine>,
    origin: Option<&'t str>,
    external_tags: HashMap<String, TagContext<'py>>,
    external_filters: HashMap<String, Bound<'py, PyAny>>,
    builtin_filters: RefCell<Option<Box<BuiltinFilterCache>>>,
    forloop_depth: usize,
    named_cycles: HashMap<String, Cycle>,
    last_cycle: Option<CycleId>,
    block_names: HashSet<String>,
    partial_names: HashSet<String>,
    has_extends: bool,
    pub last_tag: Option<At>,
    pub warning_origin: Option<String>,
    pub emit_warnings: bool,
    pub inspect_only: bool,
    pub python_nodes: HashMap<usize, (usize, PythonNode)>,
    skip_until: usize,
    pub compiler_parser: Option<Py<PyAny>>,
    compiler_loads: Vec<At>,
    registered_compilers: Option<HashMap<String, Bound<'py, PyAny>>>,
    mixed_compilers: Option<bool>,
}

impl<'t, 'py> Parser<'t, 'py> {
    pub fn new(
        py: Python<'py>,
        template: TemplateString<'t>,
        engine: Gc<Engine>,
        origin: Option<&'t str>,
    ) -> Self {
        Self {
            py,
            template,
            lexer: Lexer::new(template)
                .with_modern_raw_blocks(engine.django_series >= (6, 2))
                .with_multiline_tags(engine.multiline_tags()),
            engine,
            origin,
            external_tags: HashMap::new(),
            external_filters: HashMap::new(),
            builtin_filters: RefCell::new(None),
            forloop_depth: 0,
            named_cycles: HashMap::new(),
            last_cycle: None,
            block_names: HashSet::new(),
            partial_names: HashSet::new(),
            has_extends: false,
            last_tag: None,
            warning_origin: origin.map(str::to_string),
            emit_warnings: true,
            inspect_only: false,
            python_nodes: HashMap::new(),
            skip_until: 0,
            compiler_parser: None,
            compiler_loads: Vec::new(),
            registered_compilers: None,
            mixed_compilers: None,
        }
    }

    #[cfg(test)]
    fn new_with_filters(
        py: Python<'py>,
        template: TemplateString<'t>,
        external_filters: HashMap<String, Bound<'py, PyAny>>,
    ) -> Self {
        let engine: Gc<Engine> = Engine::empty().into();
        Self {
            py,
            template,
            lexer: Lexer::new(template)
                .with_modern_raw_blocks(engine.django_series >= (6, 2))
                .with_multiline_tags(engine.multiline_tags()),
            engine,
            origin: None,
            external_tags: HashMap::new(),
            external_filters,
            builtin_filters: RefCell::new(None),
            forloop_depth: 0,
            named_cycles: HashMap::new(),
            last_cycle: None,
            block_names: HashSet::new(),
            partial_names: HashSet::new(),
            has_extends: false,
            last_tag: None,
            warning_origin: None,
            emit_warnings: true,
            inspect_only: false,
            python_nodes: HashMap::new(),
            skip_until: 0,
            compiler_parser: None,
            compiler_loads: Vec::new(),
            registered_compilers: None,
            mixed_compilers: None,
        }
    }

    fn builtin_filter(&self, builtin: BuiltinFilter, at: At) -> Result<Gc<Py<PyAny>>, ParseError> {
        static DEFAULT_FILTERS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
        let name = builtin.name();
        if let Some(cache) = self.builtin_filters.borrow().as_ref()
            && let Some(filter) = &cache[builtin.index()]
        {
            return Ok(filter.clone());
        }
        let lookup = (|| -> PyResult<_> {
            let library = match self.engine.template_builtins.get(1) {
                Some(library) => library.bind(self.py),
                None => {
                    DEFAULT_FILTERS.import(self.py, "django.template.defaultfilters", "register")?
                }
            };
            library.getattr(intern!(self.py, "filters"))?.get_item(name)
        })();
        let filter = Gc::new(
            lookup
                // A built-in missing from the filter library is what Django
                // reports as an invalid filter, so the lookup error isn't needed.
                .map_err(|_missing_filter| ParseError::InvalidFilter {
                    at: at.into(),
                    filter: name.to_string(),
                })?
                .unbind(),
        );
        self.builtin_filters
            .borrow_mut()
            .get_or_insert_with(|| Box::new([const { None }; BuiltinFilter::COUNT]))
            [builtin.index()] = Some(filter.clone());
        Ok(filter)
    }

    pub fn parse(&mut self) -> Result<Vec<TokenTree>, PyParseError> {
        let libraries = self
            .engine
            .template_builtins
            .iter()
            .skip(3)
            .map(|library| library.bind(self.py).clone())
            .collect::<Vec<_>>();
        for library in libraries {
            self.external_filters.extend(self.get_filters(&library)?);
            for (name, tag) in self.get_tags(&library)? {
                self.register_tag(&name, tag)?;
            }
        }
        self.parse_nodes()
    }

    fn check_extends_first(nodes: &[TokenTree], node: &TokenTree) -> Result<(), PyParseError> {
        if matches!(node.as_tag(), Some(Tag::Extends { .. }))
            && nodes.iter().any(|node| !matches!(node, TokenTree::Text(_)))
        {
            return Err(
                crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                    "{% extends %} must be the first tag in the template.",
                )
                .into(),
            );
        }
        Ok(())
    }

    fn parse_nodes(&mut self) -> Result<Vec<TokenTree>, PyParseError> {
        let mut nodes = Vec::new();
        while let Some(token) = self.lexer.next() {
            if token.at.0 < self.skip_until {
                continue;
            }
            if let Some((end, node)) = self.python_nodes.get(&token.at.0) {
                self.skip_until = *end;
                nodes.push(TokenTree::tag(Tag::PythonNode(node.clone())));
                continue;
            }
            let node = match token.token_type {
                TokenType::Text => TokenTree::Text(Text::new(token.at)),
                TokenType::Comment => continue,
                TokenType::Variable => self.parse_variable_output(
                    token.content(self.template),
                    token.at,
                    token.trimmed_at().0,
                )?,
                TokenType::Tag => match self.parse_tag(token.content(self.template), token.at)? {
                    Either::Left(token_tree) => token_tree,
                    Either::Right(end_tag) => {
                        return Err(ParseError::UnexpectedEndTag {
                            at: end_tag.at.into(),
                            unexpected: end_tag.as_cow(),
                        }
                        .into());
                    }
                },
            };
            Self::check_extends_first(&nodes, &node)?;
            nodes.push(node);
        }
        // A compiled template lives as long as its loader's cache, so give
        // back the room left over from growing the list.
        nodes.shrink_to_fit();
        Ok(nodes)
    }

    fn parse_until(
        &mut self,
        until: Vec<EndTagType>,
        start: Cow<'static, str>,
        start_at: At,
    ) -> Result<(Vec<TokenTree>, EndTag), PyParseError> {
        let mut nodes = Vec::new();
        while let Some(token) = self.lexer.next() {
            if token.at.0 < self.skip_until {
                continue;
            }
            if let Some((end, node)) = self.python_nodes.get(&token.at.0) {
                self.skip_until = *end;
                nodes.push(TokenTree::tag(Tag::PythonNode(node.clone())));
                continue;
            }
            let node = match token.token_type {
                TokenType::Text => TokenTree::Text(Text::new(token.at)),
                TokenType::Comment => continue,
                TokenType::Variable => self.parse_variable_output(
                    token.content(self.template),
                    token.at,
                    token.trimmed_at().0,
                )?,
                TokenType::Tag => match self.parse_tag(token.content(self.template), token.at)? {
                    Either::Left(token_tree) => token_tree,
                    Either::Right(end_tag) => {
                        if until.contains(&end_tag.end) {
                            nodes.shrink_to_fit();
                            return Ok((nodes, end_tag));
                        }
                        return Err(ParseError::WrongEndTag {
                            expected: until
                                .iter()
                                .map(EndTagType::as_cow)
                                .collect::<Vec<_>>()
                                .join(", "),
                            unexpected: end_tag.as_cow(),
                            at: end_tag.at.into(),
                            start_at: start_at.into(),
                        }
                        .into());
                    }
                },
            };
            Self::check_extends_first(&nodes, &node)?;
            nodes.push(node);
        }
        Err(ParseError::MissingEndTag {
            start,
            expected: until
                .iter()
                .map(EndTagType::as_cow)
                .collect::<Vec<_>>()
                .join(", ")
                .into(),
            at: start_at.into(),
        }
        .into())
    }

    fn parse_variable_output(
        &self,
        source: &str,
        at: At,
        start: usize,
    ) -> Result<TokenTree, PyParseError> {
        let expression = self.parse_filter_expression(source, at, start)?;
        if self.emit_warnings && self.engine.django_series >= (6, 1) {
            let mut root = &expression;
            while let TagElement::Filter(filter) = root {
                root = &filter.left;
            }
            if matches!(root, TagElement::Variable(_) | TagElement::ForVariable(_))
                && source
                    .split('|')
                    .next()
                    .is_some_and(|root| root.contains(".."))
            {
                self.py
                    .import("django_rusty_templates.diagnostics")?
                    .getattr("warn_double_dot")?
                    .call1((self.template.0, at.0, self.warning_origin.as_deref()))?;
            }
        }
        Ok(expression.into())
    }

    fn parse_variable_reference(&self, at: At) -> Either<Variable, ForVariable> {
        let mut parts = self.template.content(at).split('.');
        if self.forloop_depth == 0
            || parts
                .next()
                .expect("a variable can always be split into at least one part")
                .trim()
                != "forloop"
        {
            return Either::Left(Variable::new(at));
        }
        let Some(part) = parts.next_back() else {
            return Either::Right(ForVariable {
                variant: ForVariableName::Object,
                parent_count: 0,
                at,
            });
        };
        let variant = match part.trim() {
            "length" => ForVariableName::Length,
            "counter" => ForVariableName::Counter,
            "counter0" => ForVariableName::Counter0,
            "revcounter" => ForVariableName::RevCounter,
            "revcounter0" => ForVariableName::RevCounter0,
            "first" => ForVariableName::First,
            "last" => ForVariableName::Last,
            "parentloop" => ForVariableName::Object,
            _ => return Either::Left(Variable::new(at)),
        };
        let parts: Vec<_> = parts.collect();
        for part in &parts {
            if part.trim() != "parentloop" {
                return Either::Left(Variable::new(at));
            }
        }
        let mut parent_count = parts.len();
        if variant == ForVariableName::Object {
            parent_count += 1;
        }
        if parent_count > self.forloop_depth {
            return Either::Left(Variable::new(at));
        }
        Either::Right(ForVariable {
            variant,
            parent_count,
            at,
        })
    }

    fn parse_filter_expression(
        &self,
        variable: &str,
        at: At,
        start: usize,
    ) -> Result<TagElement, ParseError> {
        let trimmed = variable.trim_start();
        let start = start + variable.len() - trimmed.len();
        let (mut var, at, filter_lexer) = if trimmed.starts_with("_(") {
            let mut chars = trimmed.chars();
            chars.next();
            chars.next();
            let (at, byte, rest) = dtl_lexer::common::lex_translated(start, trimmed, &mut chars)?;
            (
                TagElement::TranslatedText(Text::new(translated_text_content_at(at))),
                at,
                FilterLexer::new(rest, byte)?
                    .with_modern_syntax(self.engine.django_series >= (6, 0)),
            )
        } else if let Some(quote @ ('"' | '\'')) = trimmed.chars().next() {
            let mut chars = trimmed.chars();
            chars.next();
            let (at, byte, rest) = dtl_lexer::common::lex_text(start, trimmed, &mut chars, quote)?;
            (
                TagElement::Text(Text::new(text_content_at(at))),
                at,
                FilterLexer::new(rest, byte)?
                    .with_modern_syntax(self.engine.django_series >= (6, 0)),
            )
        } else {
            let Some((token, at, filters)) = lex_variable_or_filter_with_syntax(
                trimmed,
                start,
                self.engine.django_series >= (6, 0),
            )?
            else {
                return Err(ParseError::EmptyVariable { at: at.into() });
            };
            let value = match token {
                VariableToken::Variable => self.parse_variable_reference(at).into(),
                VariableToken::Int(n) => TagElement::Int(n),
                VariableToken::Float(f) => TagElement::Float(f),
            };
            (value, at, filters)
        };
        for filter_token in filter_lexer {
            let filter_token = filter_token?;
            let argument = match filter_token.argument {
                None => None,
                Some(ref a) => Some(a.parse(self)?),
            };
            let filter_at = (at.0, filter_token.at.0 - at.0 + filter_token.at.1);
            let filter = Filter::new(self, filter_token.at, filter_at, var, argument)?;
            var = TagElement::Filter(Box::new(filter));
        }
        Ok(var)
    }

    /// The text Django compares when checking a named closing tag.
    /// Django 6.2 joins the tag's words with single spaces, so the name may
    /// sit on its own line.
    fn end_tag_source(&self, at: At) -> Cow<'t, str> {
        let source = self.template.content(at);
        let source = source[START_TAG_LEN..source.len() - START_TAG_LEN].trim();
        if self.engine.django_series >= (6, 2) {
            Cow::Owned(source.split_whitespace().collect::<Vec<_>>().join(" "))
        } else {
            Cow::Borrowed(source)
        }
    }

    fn argument_spans(&self, parts: TagParts) -> Vec<At> {
        let content = self.template.content(parts.at);
        let mut tokens = Vec::new();
        let mut start = None;
        let mut quote = None;
        let mut escaped = false;
        for (offset, character) in content.char_indices() {
            if let Some(end) = quote {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == end {
                    quote = None;
                }
            } else if matches!(character, '\'' | '"') {
                start.get_or_insert(offset);
                quote = Some(character);
            } else if character.is_whitespace() {
                if let Some(begin) = start.take() {
                    tokens.push((parts.at.0 + begin, offset - begin));
                }
            } else {
                start.get_or_insert(offset);
            }
        }
        if let Some(begin) = start {
            tokens.push((parts.at.0 + begin, content.len() - begin));
        }
        tokens
    }

    /// `{% else %}`, `{% endif %}` and `{% empty %}` take no arguments:
    /// Django compares the whole tag with the name.
    fn reject_arguments(&self, parts: &TagParts) -> Result<(), PyParseError> {
        if self.template.content(parts.at).trim().is_empty() {
            return Ok(());
        }
        Err(
            crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                "Malformed template tag",
            )
            .into(),
        )
    }

    #[expect(clippy::too_many_lines, reason = "one match arm per builtin tag")]
    fn parse_tag(
        &mut self,
        tag: &'t str,
        at: At,
    ) -> Result<Either<TokenTree, EndTag>, PyParseError> {
        self.last_tag = Some(at);
        let tag = lex_tag(tag, at.0 + START_TAG_LEN).map_err(ParseError::from)?;

        Ok(match tag.content(self.template) {
            "block" => Either::Left(self.parse_block(at, tag.parts)?),
            "extends" => Either::Left(self.parse_extends(at, tag.parts)?),
            "endblock" => Either::Right(EndTag {
                end: EndTagType::Custom("endblock".to_string()),
                at,
                parts: Some(tag.parts),
            }),
            "resetcycle" => Either::Left(self.parse_resetcycle(tag.parts)?),
            "with" => Either::Left(self.parse_with(at, tag.parts)?),
            "filter" => Either::Left(self.parse_filter_block(at, tag.parts)?),
            "debug" => Either::Left(TokenTree::tag(Tag::Debug)),
            "spaceless" | "verbatim" => {
                let name = tag.content(self.template);
                let end = if name == "verbatim" {
                    EndTagType::Verbatim
                } else {
                    EndTagType::Custom("endspaceless".to_string())
                };
                let (nodes, _) = self.parse_until(vec![end], name.to_string().into(), at)?;
                Either::Left(TokenTree::tag(if name == "verbatim" {
                    Tag::Verbatim(nodes)
                } else {
                    Tag::Spaceless(nodes)
                }))
            }
            name @ ("endwith" | "endfilter" | "endspaceless") => Either::Right(EndTag {
                end: EndTagType::Custom(name.to_string()),
                at,
                parts: None,
            }),
            "csp_nonce_attr" if self.engine.django_series >= (6, 1) => {
                let compile = self
                    .py
                    .import("django.template.defaulttags")?
                    .getattr("register")?
                    .getattr("tags")?
                    .get_item("csp_nonce_attr")?;
                self.load_tag(at, "csp_nonce_attr", &compile)?;
                let Some(TagContext::Simple(context)) = self.external_tags.get("csp_nonce_attr")
                else {
                    unreachable!("csp_nonce_attr is a simple tag")
                };
                Either::Left(self.parse_simple_tag(context, at, tag.parts)?)
            }
            "querystring" => {
                let compile = self
                    .py
                    .import("django.template.defaulttags")?
                    .getattr("register")?
                    .getattr("tags")?
                    .get_item("querystring")?;
                self.load_tag(at, "querystring", &compile)?;
                let Some(TagContext::Simple(context)) = self.external_tags.get("querystring")
                else {
                    unreachable!("querystring is a simple tag")
                };
                let Some(Tag::SimpleTag(simple)) =
                    self.parse_simple_tag(context, at, tag.parts)?.into_tag()
                else {
                    unreachable!("querystring parser returns a simple tag")
                };
                Either::Left(TokenTree::tag(Tag::QueryString(simple)))
            }
            "regroup" => Either::Left(self.parse_regroup(tag.parts)?),
            "ifchanged" => Either::Left(self.parse_ifchanged(at, tag.parts)?),
            "endifchanged" => Either::Right(EndTag {
                end: EndTagType::Custom("endifchanged".to_string()),
                at,
                parts: None,
            }),
            "widthratio" => Either::Left(self.parse_widthratio(tag.parts)?),
            "url" => Either::Left(self.parse_url(at, tag.parts)?),
            "firstof" => Either::Left(self.parse_firstof(tag.parts)?),
            "csrf_token" => Either::Left(TokenTree::tag(Tag::CsrfToken(CsrfToken))),
            "load" => Either::Left(self.parse_load(at, tag.parts)?),
            "autoescape" => Either::Left(self.parse_autoescape(at, tag.parts)?),
            "endautoescape" => Either::Right(EndTag {
                end: EndTagType::Autoescape,
                at,
                parts: None,
            }),
            "endverbatim" => Either::Right(EndTag {
                end: EndTagType::Verbatim,
                at,
                parts: None,
            }),
            "if" => Either::Left(self.parse_if(at, tag.parts, "if")?),
            "elif" => Either::Right(EndTag {
                end: EndTagType::Elif,
                at,
                parts: Some(tag.parts),
            }),
            "else" => {
                self.reject_arguments(&tag.parts)?;
                Either::Right(EndTag {
                    end: EndTagType::Else,
                    at,
                    parts: None,
                })
            }
            "endif" => {
                self.reject_arguments(&tag.parts)?;
                Either::Right(EndTag {
                    end: EndTagType::EndIf,
                    at,
                    parts: None,
                })
            }
            "for" => Either::Left(self.parse_for(at, tag.parts)?),
            "empty" => {
                self.reject_arguments(&tag.parts)?;
                Either::Right(EndTag {
                    end: EndTagType::Empty,
                    at,
                    parts: None,
                })
            }
            "endfor" => Either::Right(EndTag {
                end: EndTagType::EndFor,
                at,
                parts: None,
            }),
            "include" => Either::Left(self.parse_include(at, tag.parts)?),
            "lorem" => Either::Left(TokenTree::tag(Tag::Lorem(self.parse_lorem(at, tag.parts)?))),
            "comment" => Either::Left(TokenTree::tag(Tag::Comment(
                self.parse_comment(at, tag.parts)?,
            ))),
            "partialdef" if self.engine.django_series >= (6, 0) => {
                Either::Left(self.parse_partialdef(at, tag.parts)?)
            }
            "partial" if self.engine.django_series >= (6, 0) => {
                let tokens = self.argument_spans(tag.parts);
                let [name] = tokens.as_slice() else {
                    return Err(
                        crate::template::django_rusty_templates::TemplateSyntaxError::new_err(
                            "'partial' tag requires a single argument",
                        )
                        .into(),
                    );
                };
                Either::Left(TokenTree::tag(Tag::Partial {
                    name: self.template.content(*name).to_string(),
                    at,
                }))
            }
            "endpartialdef" if self.engine.django_series >= (6, 0) => Either::Right(EndTag {
                end: EndTagType::Custom("endpartialdef".to_string()),
                at,
                parts: Some(tag.parts),
            }),
            name
            @ ("endcache" | "endlocalize" | "endlocaltime" | "endtimezone" | "endlanguage") => {
                Either::Right(EndTag {
                    end: EndTagType::Custom(name.to_string()),
                    at,
                    parts: None,
                })
            }
            "now" => Either::Left(TokenTree::tag(Tag::Now(self.parse_now(tag.parts)?))),
            "templatetag" => Either::Left(TokenTree::tag(Tag::TemplateTag(
                lex_templatetag(self.template, tag.parts).map_err(ParseError::from)?,
            ))),
            "cycle" => Either::Left(TokenTree::tag(Tag::Cycle(self.parse_cycle(tag.parts)?))),
            tag_name => {
                if let Some(TagContext::Unloaded(tag)) = self.external_tags.get(tag_name).cloned() {
                    self.load_tag(at, tag_name, &tag)?;
                }
                match self.external_tags.get(tag_name) {
                    Some(TagContext::Opaque(compiler)) => {
                        let compiler = compiler.clone();
                        Either::Left(self.parse_opaque(at, &compiler)?)
                    }
                    Some(TagContext::Stock(kind)) => {
                        let kind = *kind;
                        Either::Left(self.parse_stock(at, tag_name, kind, tag.parts)?)
                    }
                    Some(TagContext::Simple(context)) => {
                        Either::Left(self.parse_simple_tag(context, at, tag.parts)?)
                    }
                    Some(TagContext::Inclusion { context, filename }) => {
                        let Some(Tag::SimpleTag(call)) =
                            self.parse_simple_tag(context, at, tag.parts)?.into_tag()
                        else {
                            unreachable!("helper parser returns a simple tag")
                        };
                        if call.target_var.is_some() {
                            return Err(crate::template::django_rusty_templates::TemplateSyntaxError::new_err(format!("'{tag_name}' received too many positional arguments")).into());
                        }
                        Either::Left(TokenTree::tag(Tag::InclusionTag(InclusionTag {
                            id: CycleId(NEXT_CYCLE_ID.fetch_add(1, Ordering::Relaxed)),
                            call,
                            filename: Gc::new(filename.clone().unbind()),
                            engine: self.engine.clone(),
                            admin_template: None,
                        })))
                    }
                    Some(TagContext::AdminInclusion {
                        context,
                        template_name,
                    }) => {
                        let template_name = *template_name;
                        let Some(Tag::SimpleTag(call)) =
                            self.parse_simple_tag(context, at, tag.parts)?.into_tag()
                        else {
                            unreachable!("helper parser returns a simple tag")
                        };
                        // Django's `parse_bits` has no `as` form, so the
                        // target counts as extra positional arguments.
                        if call.target_var.is_some() {
                            return Err(crate::template::django_rusty_templates::TemplateSyntaxError::new_err(format!("'{tag_name}' received too many positional arguments")).into());
                        }
                        Either::Left(TokenTree::tag(Tag::InclusionTag(InclusionTag {
                            id: CycleId(NEXT_CYCLE_ID.fetch_add(1, Ordering::Relaxed)),
                            call,
                            filename: Gc::new(self.py.None()),
                            engine: self.engine.clone(),
                            admin_template: Some(template_name),
                        })))
                    }
                    Some(TagContext::SimpleBlock {
                        context,
                        end_tag_name,
                    }) => Either::Left(self.parse_simple_block_tag(
                        context.clone(),
                        tag_name.to_string(),
                        end_tag_name.clone(),
                        at,
                        tag.parts,
                    )?),
                    Some(TagContext::EndSimpleBlock) => Either::Right(EndTag {
                        end: EndTagType::Custom(tag_name.to_string()),
                        at,
                        parts: None,
                    }),
                    Some(TagContext::Unloaded(_)) => unreachable!("tag was loaded above"),
                    None => return Err(Unimplemented::Tag(tag_name.to_string()).into()),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::FilterType;
    use dtl_lexer::tag::TagLexerError;
    use num_bigint::BigInt;
    use pyo3::types::{PyDict, PyDictMethods};

    /// Text and variable nodes are most of a template, so the node type
    /// must stay small; large payloads belong behind a `Box`.
    #[test]
    fn test_token_tree_stays_small() {
        assert!(std::mem::size_of::<TokenTree>() <= 40);
    }

    use crate::{
        filters::{DefaultFilter, ExternalFilter, LowerFilter},
        template::django_rusty_templates::{Engine, Template},
    };
    use dtl_lexer::common::LexerError;

    fn get_external_filter(node: &TokenTree) -> Gc<Py<PyAny>> {
        match node {
            TokenTree::Filter(filter) => match &filter.filter {
                FilterType::External(filter) => filter.filter.clone(),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    fn get_external_filter_tag_element(node: &TokenTree) -> Gc<Py<PyAny>> {
        match node {
            TokenTree::Filter(filter) => match &filter.left {
                TagElement::Filter(filter) => match &filter.filter {
                    FilterType::External(filter) => filter.filter.clone(),
                    _ => panic!(),
                },
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn test_empty_template() {
        Python::initialize();

        Python::attach(|py| {
            let template = "";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();
            assert_eq!(nodes, vec![]);
        });
    }

    #[test]
    fn test_text() {
        Python::initialize();

        Python::attach(|py| {
            let template = "Some text";
            let template_string = TemplateString(template);
            let mut parser = Parser::new(py, template_string, Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();
            let text = Text::new((0, template.len()));
            assert_eq!(nodes, vec![TokenTree::Text(text)]);
            assert_eq!(template_string.content(text.at), template);
        });
    }

    #[test]
    fn test_comment() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{# A comment #}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();
            assert_eq!(nodes, vec![]);
        });
    }

    #[test]
    fn test_empty_variable() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{{ }}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(error, ParseError::EmptyVariable { at: (0, 5).into() });
        });
    }

    #[test]
    fn test_variable() {
        Python::initialize();

        Python::attach(|py| {
            let template = TemplateString("{{ foo }}");
            let mut parser = Parser::new(py, template, Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();
            let variable = Variable { at: (3, 3) };
            assert_eq!(nodes, vec![TokenTree::Variable(variable)]);
            assert_eq!(
                variable.parts(template).collect::<Vec<_>>(),
                vec![("foo", (3, 3))]
            );
        });
    }

    #[test]
    fn test_variable_attribute() {
        Python::initialize();

        Python::attach(|py| {
            let template = TemplateString("{{ foo.bar.baz }}");
            let mut parser = Parser::new(py, template, Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();
            let variable = Variable { at: (3, 11) };
            assert_eq!(nodes, vec![TokenTree::Variable(variable)]);
            assert_eq!(
                variable.parts(template).collect::<Vec<_>>(),
                vec![("foo", (3, 3)), ("bar", (7, 3)), ("baz", (11, 3))]
            );
        });
    }

    #[test]
    fn test_filter() {
        Python::initialize();

        Python::attach(|py| {
            let filters = HashMap::from([("bar".to_string(), py.None().bind(py).clone())]);
            let template = TemplateString("{{ foo|bar }}");
            let mut parser = Parser::new_with_filters(py, template, filters);
            let nodes = parser.parse().unwrap();

            assert_eq!(nodes.len(), 1);

            let foo = Variable { at: (3, 3) };
            let external = get_external_filter(&nodes[0]);
            assert!(external.is_none(py));
            let bar = TokenTree::Filter(Box::new(Filter {
                at: (7, 3),
                all_at: (3, 7),
                left: TagElement::Variable(foo),
                filter: FilterType::External(ExternalFilter {
                    filter: external,
                    argument: None,
                }),
            }));
            assert_eq!(nodes, vec![bar]);
            assert_eq!(
                foo.parts(template).collect::<Vec<_>>(),
                vec![("foo", (3, 3))]
            );
        });
    }

    #[test]
    fn test_unknown_filter() {
        Python::initialize();

        Python::attach(|py| {
            let template = TemplateString("{{ foo|bar }}");
            let mut parser = Parser::new(py, template, Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(
                error,
                ParseError::InvalidFilter {
                    filter: "bar".to_string(),
                    at: (7, 3).into()
                }
            );
        });
    }

    #[test]
    fn test_filter_multiple() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{{ foo|bar|baz }}";
            let filters = HashMap::from([
                ("bar".to_string(), py.None().bind(py).clone()),
                ("baz".to_string(), py.None().bind(py).clone()),
            ]);
            let mut parser = Parser::new_with_filters(py, template.into(), filters);
            let nodes = parser.parse().unwrap();
            assert_eq!(nodes.len(), 1);

            let foo = TagElement::Variable(Variable { at: (3, 3) });
            let external = get_external_filter_tag_element(&nodes[0]);
            assert!(external.is_none(py));
            let bar = TagElement::Filter(Box::new(Filter {
                at: (7, 3),
                all_at: (3, 7),
                left: foo,
                filter: FilterType::External(ExternalFilter {
                    filter: external,
                    argument: None,
                }),
            }));
            let external = get_external_filter(&nodes[0]);
            assert!(external.is_none(py));
            let baz = TokenTree::Filter(Box::new(Filter {
                at: (11, 3),
                all_at: (3, 11),
                left: bar,
                filter: FilterType::External(ExternalFilter {
                    filter: external,
                    argument: None,
                }),
            }));
            assert_eq!(nodes, vec![baz]);
        });
    }

    #[test]
    fn test_filter_argument() {
        Python::initialize();

        Python::attach(|py| {
            let filters = HashMap::from([("bar".to_string(), py.None().bind(py).clone())]);
            let template = TemplateString("{{ foo|bar:baz }}");
            let mut parser = Parser::new_with_filters(py, template, filters);
            let nodes = parser.parse().unwrap();
            assert_eq!(nodes.len(), 1);

            let foo = TagElement::Variable(Variable { at: (3, 3) });
            let baz = Variable { at: (11, 3) };
            let external = get_external_filter(&nodes[0]);
            assert!(external.is_none(py));
            let bar = TokenTree::Filter(Box::new(Filter {
                at: (7, 3),
                all_at: (3, 7),
                left: foo,
                filter: FilterType::External(ExternalFilter {
                    filter: external,
                    argument: Some(Argument {
                        at: (11, 3),
                        argument_type: ArgumentType::Variable(baz),
                    }),
                }),
            }));
            assert_eq!(nodes, vec![bar]);
            assert_eq!(
                baz.parts(template).collect::<Vec<_>>(),
                vec![("baz", (11, 3))]
            );
        });
    }

    #[test]
    fn test_filter_argument_text() {
        Python::initialize();

        Python::attach(|py| {
            let filters = HashMap::from([("bar".to_string(), py.None().bind(py).clone())]);
            let template = TemplateString("{{ foo|bar:'baz' }}");
            let mut parser = Parser::new_with_filters(py, template, filters);
            let nodes = parser.parse().unwrap();

            let foo = TagElement::Variable(Variable { at: (3, 3) });
            let baz = Text::new((12, 3));
            let external = get_external_filter(&nodes[0]);
            assert!(external.is_none(py));
            let bar = TokenTree::Filter(Box::new(Filter {
                at: (7, 3),
                all_at: (3, 7),
                left: foo,
                filter: FilterType::External(ExternalFilter {
                    filter: external,
                    argument: Some(Argument {
                        at: (11, 5),
                        argument_type: ArgumentType::Text(baz),
                    }),
                }),
            }));
            assert_eq!(nodes, vec![bar]);
            assert_eq!(template.content(baz.at), "baz");
        });
    }

    #[test]
    fn test_filter_argument_translated_text() {
        Python::initialize();

        Python::attach(|py| {
            let filters = HashMap::from([("bar".to_string(), py.None().bind(py).clone())]);
            let template = TemplateString("{{ foo|bar:_('baz') }}");
            let mut parser = Parser::new_with_filters(py, template, filters);
            let nodes = parser.parse().unwrap();

            let foo = TagElement::Variable(Variable { at: (3, 3) });
            let baz = TranslatedText::new((14, 3));
            let external = get_external_filter(&nodes[0]);
            assert!(external.is_none(py));
            let bar = TokenTree::Filter(Box::new(Filter {
                at: (7, 3),
                all_at: (3, 7),
                left: foo,
                filter: FilterType::External(ExternalFilter {
                    filter: external,
                    argument: Some(Argument {
                        at: (11, 8),
                        argument_type: ArgumentType::TranslatedText(baz),
                    }),
                }),
            }));
            assert_eq!(nodes, vec![bar]);
            assert_eq!(template.content(baz.at), "baz");
        });
    }

    #[test]
    fn test_filter_argument_float() {
        Python::initialize();

        Python::attach(|py| {
            let filters = HashMap::from([("bar".to_string(), py.None().bind(py).clone())]);
            let template = "{{ foo|bar:5.2e3 }}";
            let mut parser = Parser::new_with_filters(py, template.into(), filters);
            let nodes = parser.parse().unwrap();

            let foo = TagElement::Variable(Variable { at: (3, 3) });
            let num = Argument {
                at: (11, 5),
                argument_type: ArgumentType::Float(5.2e3),
            };
            let external = get_external_filter(&nodes[0]);
            assert!(external.is_none(py));
            let bar = TokenTree::Filter(Box::new(Filter {
                at: (7, 3),
                all_at: (3, 7),
                left: foo,
                filter: FilterType::External(ExternalFilter {
                    filter: external,
                    argument: Some(num),
                }),
            }));
            assert_eq!(nodes, vec![bar]);
        });
    }

    #[test]
    fn test_filter_argument_int() {
        Python::initialize();

        Python::attach(|py| {
            let filters = HashMap::from([("bar".to_string(), py.None().bind(py).clone())]);
            let template = "{{ foo|bar:99 }}";
            let mut parser = Parser::new_with_filters(py, template.into(), filters);
            let nodes = parser.parse().unwrap();

            let foo = TagElement::Variable(Variable { at: (3, 3) });
            let num = Argument {
                at: (11, 2),
                argument_type: ArgumentType::Int(99.into()),
            };
            let external = get_external_filter(&nodes[0]);
            assert!(external.is_none(py));
            let bar = TokenTree::Filter(Box::new(Filter {
                at: (7, 3),
                all_at: (3, 7),
                left: foo,
                filter: FilterType::External(ExternalFilter {
                    filter: external,
                    argument: Some(num),
                }),
            }));
            assert_eq!(nodes, vec![bar]);
        });
    }

    #[test]
    fn test_filter_argument_bigint() {
        Python::initialize();

        Python::attach(|py| {
            let filters = HashMap::from([("bar".to_string(), py.None().bind(py).clone())]);
            let template = "{{ foo|bar:99999999999999999 }}";
            let mut parser = Parser::new_with_filters(py, template.into(), filters);
            let nodes = parser.parse().unwrap();

            let foo = TagElement::Variable(Variable { at: (3, 3) });
            let num = Argument {
                at: (11, 17),
                argument_type: ArgumentType::Int("99999999999999999".parse::<BigInt>().unwrap()),
            };
            let external = get_external_filter(&nodes[0]);
            assert!(external.is_none(py));
            let bar = TokenTree::Filter(Box::new(Filter {
                at: (7, 3),
                all_at: (3, 7),
                left: foo,
                filter: FilterType::External(ExternalFilter {
                    filter: external,
                    argument: Some(num),
                }),
            }));
            assert_eq!(nodes, vec![bar]);
        });
    }

    #[test]
    fn test_filter_argument_invalid_number() {
        Python::initialize();

        Python::attach(|py| {
            // `9.9.9` would be a variable lookup, as it is in Django.
            let template = "{{ foo|bar:-. }}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(error, ParseError::InvalidNumber { at: (11, 2).into() });
        });
    }

    #[test]
    fn test_filter_parse_addslashes() {
        Python::initialize();

        Python::attach(|py| {
            let engine = Gc::new(Engine::empty());
            let template_string = "{{ foo|addslashes }}".to_string();
            let context = PyDict::new(py);
            context.set_item("bar", "").unwrap();
            let template = Template::new_from_string(py, template_string, engine.clone()).unwrap();
            let result = template
                .py_render(py, Some(context.into_any()), None)
                .unwrap();

            assert_eq!(result, "");

            let context = PyDict::new(py);
            context.set_item("foo", "").unwrap();
            let template_string = "{{ foo|addslashes:invalid }}".to_string();
            let error = Template::new_from_string(py, template_string, engine).unwrap_err();

            let error_string = error
                .value(py)
                .getattr("rusty_diagnostic")
                .unwrap()
                .to_string();
            assert!(error_string.contains("addslashes filter does not take an argument"));
        });
    }

    #[test]
    fn test_filter_default() {
        Python::initialize();

        Python::attach(|py| {
            let template = TemplateString("{{ foo|default:baz }}");
            let mut parser = Parser::new(py, template, Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let foo = TagElement::Variable(Variable { at: (3, 3) });
            let baz = Variable { at: (15, 3) };
            let bar = TokenTree::Filter(Box::new(Filter {
                at: (7, 7),
                all_at: (3, 11),
                left: foo,
                filter: FilterType::Default(DefaultFilter::new(
                    Argument {
                        at: (15, 3),
                        argument_type: ArgumentType::Variable(baz),
                    },
                    (7, 7),
                )),
            }));
            assert_eq!(nodes, vec![bar]);
            assert_eq!(
                baz.parts(template).collect::<Vec<_>>(),
                vec![("baz", (15, 3))]
            );
        });
    }

    #[test]
    fn test_filter_default_missing_argument() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{{ foo|default|baz }}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(error, ParseError::MissingArgument { at: (7, 7).into() });
        });
    }

    #[test]
    fn test_filter_lower_unexpected_argument() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{{ foo|lower:baz }}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(
                error,
                ParseError::UnexpectedArgument {
                    filter: "lower",
                    at: (13, 3).into()
                }
            );
        });
    }

    #[test]
    fn test_variable_lexer_error() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{{ _foo }}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(
                error,
                ParseError::VariableError(
                    LexerError::InvalidVariableName { at: (3, 4).into() }.into()
                )
            );
        });
    }

    #[test]
    fn test_parse_empty_tag() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{%  %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(
                error,
                ParseError::BlockError(TagLexerError::EmptyTag { at: (0, 6).into() })
            );
        });
    }

    #[test]
    fn test_block_error() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url'foo' %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(
                error,
                ParseError::BlockError(TagLexerError::InvalidTagName { at: (3, 8).into() })
            );
        });
    }

    #[test]
    fn test_parse_url_tag() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url 'some-url-name' %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 25),
                view_name: TagElement::Text(Text { at: (8, 13) }),
                args: vec![],
                kwargs: vec![],
                asvar: None,
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_tag_view_name_translated() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url _('some-url-name') %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 28),
                view_name: TagElement::TranslatedText(Text { at: (10, 13) }),
                args: vec![],
                kwargs: vec![],
                asvar: None,
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_tag_view_name_variable() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url some_view_name %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 24),
                view_name: TagElement::Variable(Variable { at: (7, 14) }),
                args: vec![],
                kwargs: vec![],
                asvar: None,
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_tag_view_name_filter() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url some_view_name|default:'home' %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let some_view_name = TagElement::Variable(Variable { at: (7, 14) });
            let home = Text { at: (31, 4) };
            let default = Box::new(Filter {
                at: (22, 7),
                all_at: (7, 22),
                left: some_view_name,
                filter: FilterType::Default(DefaultFilter::new(
                    Argument {
                        at: (30, 6),
                        argument_type: ArgumentType::Text(home),
                    },
                    (22, 7),
                )),
            });
            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 39),
                view_name: TagElement::Filter(default),
                args: vec![],
                kwargs: vec![],
                asvar: None,
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_no_arguments() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(error, ParseError::UrlTagNoArguments { at: (0, 9).into() });
        });
    }

    #[test]
    fn test_parse_url_view_name_integer() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url 64 %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 12),
                view_name: TagElement::Int(64.into()),
                args: vec![],
                kwargs: vec![],
                asvar: None,
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_tag_arguments() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url some_view_name 'foo' bar|default:'home' 64 5.7 _(\"spam\") %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 66),
                view_name: TagElement::Variable(Variable { at: (7, 14) }),
                args: vec![
                    TagElement::Text(Text { at: (23, 3) }),
                    TagElement::Filter(Box::new(Filter {
                        at: (32, 7),
                        all_at: (28, 11),
                        left: TagElement::Variable(Variable { at: (28, 3) }),
                        filter: FilterType::Default(DefaultFilter::new(
                            Argument {
                                at: (40, 6),
                                argument_type: ArgumentType::Text(Text { at: (41, 4) }),
                            },
                            (32, 7),
                        )),
                    })),
                    TagElement::Int(64.into()),
                    TagElement::Float(5.7),
                    TagElement::TranslatedText(Text { at: (57, 4) }),
                ],
                kwargs: vec![],
                asvar: None,
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_tag_kwargs() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url some_view_name foo='foo' extra=-64 %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 44),
                view_name: TagElement::Variable(Variable { at: (7, 14) }),
                args: vec![],
                kwargs: vec![
                    ("foo".to_string(), TagElement::Text(Text { at: (27, 3) })),
                    ("extra".to_string(), TagElement::Int((-64).into())),
                ],
                asvar: None,
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_tag_arguments_as_variable() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url some_view_name 'foo' as some_url %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 42),
                view_name: TagElement::Variable(Variable { at: (7, 14) }),
                args: vec![TagElement::Text(Text { at: (23, 3) })],
                kwargs: vec![],
                asvar: Some("some_url".to_string()),
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_tag_kwargs_as_variable() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url some_view_name foo='foo' as some_url %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 46),
                view_name: TagElement::Variable(Variable { at: (7, 14) }),
                args: vec![],
                kwargs: vec![("foo".to_string(), TagElement::Text(Text { at: (27, 3) }))],
                asvar: Some("some_url".to_string()),
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_tag_arguments_last_variables() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url some_view_name 'foo' arg arg2 %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let nodes = parser.parse().unwrap();

            let url = TokenTree::tag(Tag::Url(Url {
                at: (0, 39),
                view_name: TagElement::Variable(Variable { at: (7, 14) }),
                args: vec![
                    TagElement::Text(Text { at: (23, 3) }),
                    TagElement::Variable(Variable { at: (28, 3) }),
                    TagElement::Variable(Variable { at: (32, 4) }),
                ],
                kwargs: vec![],
                asvar: None,
            }));

            assert_eq!(nodes, vec![url]);
        });
    }

    #[test]
    fn test_parse_url_tag_invalid_number() {
        Python::initialize();

        Python::attach(|py| {
            let template = "{% url foo 9.9.9 %}";
            let mut parser = Parser::new(py, template.into(), Engine::empty().into(), None);
            let error = parser.parse().unwrap_err().unwrap_parse_error();
            assert_eq!(error, ParseError::InvalidNumber { at: (11, 5).into() });
        });
    }

    #[test]
    fn test_filter_type_partial_eq() {
        Python::initialize();

        Python::attach(|py| {
            assert_eq!(
                FilterType::Lower(LowerFilter),
                FilterType::Lower(LowerFilter)
            );
            assert_ne!(
                FilterType::External(ExternalFilter::new(py.None(), None)),
                FilterType::External(ExternalFilter::new(py.None(), None))
            );
            assert_ne!(
                FilterType::Lower(LowerFilter),
                FilterType::Default(DefaultFilter::new(
                    Argument {
                        at: (0, 3),
                        argument_type: ArgumentType::Float(1.0)
                    },
                    (0, 7)
                ))
            );
        });
    }

    #[test]
    fn test_simple_tag_partial_eq() {
        Python::initialize();

        Python::attach(|py| {
            let func: Gc<Py<PyAny>> = PyDict::new(py).into_any().unbind().into();
            let at = (0, 1);
            let takes_context = true;
            assert_eq!(
                SimpleTag {
                    func: func.clone(),
                    at,
                    takes_context,
                    args: Vec::new(),
                    kwargs: Vec::new(),
                    target_var: Some("foo".to_string()),
                },
                SimpleTag {
                    func,
                    at,
                    takes_context,
                    args: Vec::new(),
                    kwargs: Vec::new(),
                    target_var: Some("foo".to_string()),
                },
            );
        });
    }

    #[test]
    fn test_simple_block_tag_partial_eq() {
        Python::initialize();

        Python::attach(|py| {
            let func: Gc<Py<PyAny>> = PyDict::new(py).into_any().unbind().into();
            let at = (0, 1);
            let takes_context = true;
            assert_eq!(
                SimpleBlockTag {
                    func: func.clone(),
                    at,
                    takes_context,
                    args: Vec::new(),
                    kwargs: Vec::new(),
                    nodes: Vec::new(),
                    target_var: Some("foo".to_string()),
                },
                SimpleBlockTag {
                    func,
                    at,
                    takes_context,
                    args: Vec::new(),
                    kwargs: Vec::new(),
                    nodes: Vec::new(),
                    target_var: Some("foo".to_string()),
                },
            );
        });
    }

    #[test]
    fn test_include_tag_partial_eq() {
        Python::initialize();

        Python::attach(|_| {
            let engine: Gc<Engine> = Engine::empty().into();
            let template_name = IncludeTemplateName::Variable(TagElement::Float(1.1));
            assert_eq!(
                Include {
                    id: CycleId(0),
                    template_name: template_name.clone(),
                    origin: None,
                    only: false,
                    kwargs: Vec::new(),
                    engine: engine.clone(),
                },
                Include {
                    id: CycleId(0),
                    template_name,
                    origin: None,
                    only: false,
                    kwargs: Vec::new(),
                    engine,
                },
            );
        });
    }
}
