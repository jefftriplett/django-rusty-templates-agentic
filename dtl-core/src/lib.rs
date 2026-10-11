//! Portable DTL subset shared by the experimental Python and browser adapters.
//! No Python or JavaScript dependencies belong in this crate.

use std::collections::HashMap;

use dtl_lexer::core::{Lexer, TokenType};
use dtl_lexer::types::TemplateString;
use serde_json::{Map, Value, json};

const MAX_INPUT: usize = 1_048_576;
const MAX_OUTPUT: usize = 1_048_576;
const MAX_DEPTH: usize = 32;
const MAX_STEPS: usize = 100_000;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
enum Atom {
    Literal(Value),
    Path(Vec<String>),
}

#[derive(Clone, Debug)]
struct Expression {
    root: Atom,
    filters: Vec<(String, Option<Atom>)>,
}

#[derive(Clone, Debug)]
enum Condition {
    Value(Expression),
    Not(Box<Self>),
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    Compare(Expression, String, Expression),
}

#[derive(Clone, Debug)]
enum Node {
    Text(String),
    Output(Expression),
    If(Vec<(Condition, Vec<Self>)>, Vec<Self>),
    For(String, Expression, bool, Vec<Self>, Vec<Self>),
    Include(Expression),
    Extends(Expression),
    Block(String, Vec<Self>),
}

struct Template {
    nodes: Vec<Node>,
    parent: Option<Expression>,
    prefix: Vec<Node>,
    blocks: HashMap<String, Vec<Node>>,
}

struct BlockBody<'a> {
    origin: &'a str,
    nodes: &'a [Node],
}

#[derive(Default)]
struct Blocks<'a> {
    chains: HashMap<String, Vec<BlockBody<'a>>>,
    inherited: bool,
}

struct BlockFrame {
    name: String,
    index: usize,
    super_output: Option<String>,
    allow_super: bool,
}

/// Templates are compiled on registration and reused across renders.
#[derive(Default)]
pub struct Engine {
    templates: HashMap<String, Template>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_template(&mut self, name: &str, source: &str) -> Result<()> {
        if source.len() > MAX_INPUT {
            return Err("Template exceeds 1 MiB".into());
        }
        let template = TemplateString(source);
        let tokens = Lexer::new(template)
            .map(|token| {
                let content = token.content(template).trim().to_owned();
                (token.token_type, content, token.at)
            })
            .collect::<Vec<_>>();
        // Text must retain its original whitespace.
        let tokens = tokens
            .into_iter()
            .map(|(kind, content, at)| {
                let content = if kind == TokenType::Text {
                    source[at.0..at.0 + at.1].to_owned()
                } else {
                    content
                };
                (kind, content, at.0)
            })
            .collect();
        let mut parser = Parser { tokens, cursor: 0 };
        let (nodes, stop) = parser
            .nodes(&[], 0)
            .map_err(|error| format!("{name}: {error}"))?;
        if stop.is_some() {
            return Err(format!("{name}: Unexpected closing tag"));
        }
        let mut blocks = HashMap::new();
        collect_blocks(&nodes, &mut blocks)?;
        let extends = nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                if let Node::Extends(parent) = node {
                    Some((index, parent))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        if extends.len() > 1 {
            return Err("Only one extends tag is allowed".into());
        }
        let (parent, prefix) = if let Some((index, parent)) = extends.first() {
            if nodes[..*index]
                .iter()
                .any(|node| !matches!(node, Node::Text(_)))
            {
                return Err("Extends must be the first non-text node".into());
            }
            (Some((*parent).clone()), nodes[..*index].to_vec())
        } else {
            (None, Vec::new())
        };
        self.templates.insert(
            name.into(),
            Template {
                nodes,
                parent,
                prefix,
                blocks,
            },
        );
        Ok(())
    }

    pub fn render(&self, name: &str, context_json: &str) -> Result<String> {
        if context_json.len() > MAX_INPUT {
            return Err("Context exceeds 1 MiB".into());
        }
        let context: Value =
            serde_json::from_str(context_json).map_err(|error| error.to_string())?;
        let Value::Object(context) = context else {
            return Err("Context must be a JSON object".into());
        };
        let mut state = State {
            scopes: vec![context],
            output: String::new(),
            steps: 0,
            block_frames: Vec::new(),
            origins: Vec::new(),
        };
        self.render_template(name, &mut state, 0)?;
        Ok(state.output)
    }

    fn render_template(&self, name: &str, state: &mut State, depth: usize) -> Result<()> {
        let mut blocks = Blocks::default();
        let mut visited = Vec::new();
        let mut current = name.to_owned();
        loop {
            if depth + visited.len() > MAX_DEPTH {
                return Err("Template nesting/include/inheritance depth exceeded".into());
            }
            if visited.contains(&current) {
                return Err(format!("Inheritance cycle at {current}"));
            }
            state.step()?;
            let (origin, template) = self
                .templates
                .get_key_value(&current)
                .ok_or_else(|| format!("Template not found: {current}"))?;
            visited.push(current.clone());
            for (name, body) in &template.blocks {
                blocks
                    .chains
                    .entry(name.clone())
                    .or_default()
                    .push(BlockBody {
                        origin,
                        nodes: body,
                    });
            }
            state.origins.push(origin.clone());
            let result = if let Some(parent) = &template.parent {
                blocks.inherited = true;
                self.render_nodes(&template.prefix, &blocks, state, depth)?;
                self.resolve_expression(parent, &blocks, state, depth, false)
                    .and_then(|resolved| {
                        resolved
                            .value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or("Parent name must be a string".into())
                    })
                    .and_then(|parent| relative_name(origin, &parent))
                    .map(Some)
            } else {
                self.render_nodes(&template.nodes, &blocks, state, depth + visited.len())
                    .map(|()| None)
            };
            state.origins.pop();
            match result? {
                Some(parent) => current = parent,
                None => return Ok(()),
            }
        }
    }

    fn render_block(
        &self,
        name: &str,
        index: usize,
        blocks: &Blocks<'_>,
        state: &mut State,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err("Block nesting depth exceeded".into());
        }
        let Some(body) = blocks.chains.get(name).and_then(|chain| chain.get(index)) else {
            return Ok(());
        };
        state.block_frames.push(BlockFrame {
            name: name.into(),
            index,
            super_output: None,
            allow_super: blocks.inherited,
        });
        state.origins.push(body.origin.into());
        let result = self.render_nodes(body.nodes, blocks, state, depth + 1);
        state.origins.pop();
        state.block_frames.pop();
        result
    }

    fn ensure_super(&self, blocks: &Blocks<'_>, state: &mut State, depth: usize) -> Result<()> {
        let Some(frame) = state.block_frames.last() else {
            return Ok(());
        };
        if !frame.allow_super {
            return Err("block.super requires an inheritance context".into());
        }
        let name = frame.name.clone();
        let index = frame.index + 1;
        let output = std::mem::take(&mut state.output);
        let result = self.render_block(&name, index, blocks, state, depth + 1);
        let super_output = std::mem::replace(&mut state.output, output);
        result?;
        if let Some(frame) = state.block_frames.last_mut() {
            frame.super_output = Some(super_output);
        }
        Ok(())
    }

    fn resolve_expression(
        &self,
        expression: &Expression,
        blocks: &Blocks<'_>,
        state: &mut State,
        depth: usize,
        ignore_failures: bool,
    ) -> Result<Resolved> {
        if expression.needs_super() {
            self.ensure_super(blocks, state, depth)?;
        }
        expression.resolve(state, ignore_failures)
    }

    fn render_nodes(
        &self,
        nodes: &[Node],
        blocks: &Blocks<'_>,
        state: &mut State,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err("Template nesting/include depth exceeded".into());
        }
        for node in nodes {
            state.step()?;
            match node {
                Node::Text(text) => state.append(text)?,
                Node::Output(expression) => {
                    let resolved =
                        self.resolve_expression(expression, blocks, state, depth, false)?;
                    let text = if resolved.missing {
                        String::new()
                    } else {
                        display(&resolved.value)?
                    };
                    state.append(&if resolved.safe { text } else { escape(&text) })?;
                }
                Node::Include(expression) => {
                    let value = self
                        .resolve_expression(expression, blocks, state, depth, false)?
                        .value;
                    let name = value.as_str().ok_or("Include name must be a string")?;
                    let origin = state.origins.last().map_or("", String::as_str);
                    let name = relative_name(origin, name)?;
                    // Includes have their own inheritance block map.
                    self.render_template(&name, state, depth + 1)?;
                }
                Node::Extends(_) => {
                    return Err("Extends is only allowed at template top level".into());
                }
                Node::Block(name, _) => self.render_block(name, 0, blocks, state, depth + 1)?,
                Node::If(branches, otherwise) => {
                    let mut selected = otherwise;
                    for (condition, body) in branches {
                        if condition.evaluate(self, blocks, state, depth)? {
                            selected = body;
                            break;
                        }
                    }
                    self.render_nodes(selected, blocks, state, depth + 1)?;
                }
                Node::For(name, expression, reversed, body, empty) => {
                    let value = self
                        .resolve_expression(expression, blocks, state, depth, true)?
                        .value;
                    let mut values = match value {
                        Value::Array(values) => values,
                        Value::String(value) => value
                            .chars()
                            .map(|c| Value::String(c.to_string()))
                            .collect(),
                        Value::Object(value) => {
                            value.into_iter().map(|(k, _)| Value::String(k)).collect()
                        }
                        Value::Null => Vec::new(),
                        _ => {
                            return Err("For loop requires an array, string, object or null".into());
                        }
                    };
                    if *reversed {
                        values.reverse();
                    }
                    let length = values.len();
                    if length == 0 {
                        self.render_nodes(empty, blocks, state, depth + 1)?;
                    }
                    let parent = state
                        .lookup(&["forloop".into()])
                        .unwrap_or_else(|| json!({}));
                    for (index, value) in values.into_iter().enumerate() {
                        state.step()?;
                        let mut scope = Map::new();
                        scope.insert(name.clone(), value);
                        scope.insert(
                            "forloop".into(),
                            json!({
                                "counter": index + 1, "counter0": index,
                                "revcounter": length - index, "revcounter0": length - index - 1,
                                "first": index == 0, "last": index + 1 == length,
                                "parentloop": parent,
                            }),
                        );
                        state.scopes.push(scope);
                        let result = self.render_nodes(body, blocks, state, depth + 1);
                        state.scopes.pop();
                        result?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn relative_name(origin: &str, name: &str) -> Result<String> {
    if !name.starts_with("./") && !name.starts_with("../") {
        return Ok(name.into());
    }
    let directory = origin
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    let combined = format!("{directory}/{name}");
    let mut parts = Vec::new();
    for part in combined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("Relative template name escapes the registry root".into());
                }
            }
            _ => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}

fn collect_blocks(nodes: &[Node], blocks: &mut HashMap<String, Vec<Node>>) -> Result<()> {
    for node in nodes {
        match node {
            Node::Block(name, body) => {
                if blocks.insert(name.clone(), body.clone()).is_some() {
                    return Err(format!("Duplicate block: {name}"));
                }
                collect_nested(body, blocks)?;
            }
            Node::If(branches, otherwise) => {
                for (_, body) in branches {
                    collect_nested(body, blocks)?;
                }
                collect_nested(otherwise, blocks)?;
            }
            Node::For(_, _, _, body, empty) => {
                collect_nested(body, blocks)?;
                collect_nested(empty, blocks)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn collect_nested(nodes: &[Node], blocks: &mut HashMap<String, Vec<Node>>) -> Result<()> {
    if nodes.iter().any(|node| matches!(node, Node::Extends(_))) {
        return Err("Extends is only allowed at template top level".into());
    }
    collect_blocks(nodes, blocks)
}

struct Parser {
    tokens: Vec<(TokenType, String, usize)>,
    cursor: usize,
}

impl Parser {
    fn nodes(&mut self, stops: &[&str], depth: usize) -> Result<(Vec<Node>, Option<String>)> {
        if depth > MAX_DEPTH {
            return Err("Template nesting depth exceeded".into());
        }
        let mut nodes = Vec::new();
        while let Some((kind, content, offset)) = self.tokens.get(self.cursor) {
            let content = content.clone();
            let offset = *offset;
            self.cursor += 1;
            match kind {
                TokenType::Text => nodes.push(Node::Text(content)),
                TokenType::Comment => {}
                TokenType::Variable => nodes.push(Node::Output(Expression::parse(&content)?)),
                TokenType::Tag => {
                    let (tag, argument) = content
                        .split_once(char::is_whitespace)
                        .unwrap_or((&content, ""));
                    let argument = argument.trim();
                    if stops.contains(&tag) {
                        return Ok((nodes, Some(content)));
                    }
                    let node = match tag {
                        "if" => {
                            let condition = Condition::parse(argument)?;
                            let (body, mut stop) =
                                self.nodes(&["elif", "else", "endif"], depth + 1)?;
                            let mut branches = vec![(condition, body)];
                            while let Some(next) =
                                stop.as_deref().and_then(|s| s.strip_prefix("elif "))
                            {
                                let condition = Condition::parse(next.trim())?;
                                let (body, next_stop) =
                                    self.nodes(&["elif", "else", "endif"], depth + 1)?;
                                branches.push((condition, body));
                                stop = next_stop;
                            }
                            let otherwise = if stop.as_deref() == Some("else") {
                                let (body, next_stop) = self.nodes(&["endif"], depth + 1)?;
                                stop = next_stop;
                                body
                            } else {
                                Vec::new()
                            };
                            if stop.as_deref() != Some("endif") {
                                return Err("Expected endif".into());
                            }
                            Node::If(branches, otherwise)
                        }
                        "for" => {
                            let words = split_words(argument)?;
                            if !(words.len() == 3 || (words.len() == 4 && words[3] == "reversed"))
                                || words[1] != "in"
                            {
                                return Err("Expected: for name in expression [reversed]".into());
                            }
                            let name = &words[0];
                            if !valid_identifier(name) {
                                return Err(
                                    "Loop unpacking/invalid loop name is unsupported".into()
                                );
                            }
                            let expression = Expression::parse(&words[2])?;
                            let (body, mut stop) = self.nodes(&["empty", "endfor"], depth + 1)?;
                            let empty = if stop.as_deref() == Some("empty") {
                                let (empty, next_stop) = self.nodes(&["endfor"], depth + 1)?;
                                stop = next_stop;
                                empty
                            } else {
                                Vec::new()
                            };
                            if stop.as_deref() != Some("endfor") {
                                return Err("Expected endfor".into());
                            }
                            Node::For(name.clone(), expression, words.len() == 4, body, empty)
                        }
                        "include" => Node::Include(Expression::parse(argument)?),
                        "extends" => Node::Extends(Expression::parse(argument)?),
                        "block" => {
                            if !valid_identifier(argument) {
                                return Err("Invalid block name".into());
                            }
                            let (body, stop) = self.nodes(&["endblock"], depth + 1)?;
                            if stop.as_deref() != Some("endblock")
                                && stop.as_deref() != Some(&format!("endblock {argument}"))
                            {
                                return Err("Endblock name does not match its block".into());
                            }
                            Node::Block(argument.into(), body)
                        }
                        _ => return Err(format!("Unsupported tag '{tag}' at byte {offset}")),
                    };
                    nodes.push(node);
                }
            }
        }
        if stops.is_empty() {
            Ok((nodes, None))
        } else {
            Err(format!("Unclosed tag; expected {}", stops.join("/")))
        }
    }
}

/// Split delimiters only outside quoted strings, honoring backslash escapes.
fn split_quoted(text: &str, delimiter: impl Fn(char) -> bool) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (index, c) in text.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
        } else if delimiter(c) {
            parts.push(text[start..index].trim().to_owned());
            start = index + c.len_utf8();
        }
    }
    if quote.is_some() {
        return Err("Unclosed quoted literal".into());
    }
    parts.push(text[start..].trim().to_owned());
    Ok(parts)
}

fn split_words(text: &str) -> Result<Vec<String>> {
    Ok(split_quoted(text, char::is_whitespace)?
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect())
}

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_alphabetic())
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

impl Atom {
    fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        if text.starts_with(['\'', '"']) {
            let quote = text.chars().next().ok_or("Empty expression")?;
            if text.len() < 2 || !text.ends_with(quote) {
                return Err("Invalid string literal".into());
            }
            let inner = &text[1..text.len() - 1];
            let mut value = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == quote {
                    return Err("Unexpected quote in literal".into());
                }
                if c == '\\' {
                    let next = chars.next().ok_or("Trailing backslash in literal")?;
                    if next != quote && next != '\\' {
                        value.push('\\');
                    }
                    value.push(next);
                } else {
                    value.push(c);
                }
            }
            return Ok(Self::Literal(Value::String(value)));
        }
        match text {
            "True" => return Ok(Self::Literal(Value::Bool(true))),
            "False" => return Ok(Self::Literal(Value::Bool(false))),
            "None" => return Ok(Self::Literal(Value::Null)),
            _ => {}
        }
        if let Ok(Value::Number(n)) = serde_json::from_str::<Value>(text) {
            return Ok(Self::Literal(Value::Number(n)));
        }
        let path = text.split('.').map(str::to_owned).collect::<Vec<_>>();
        if path.is_empty()
            || !path.iter().all(|part| {
                valid_identifier(part)
                    || (!part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
            })
        {
            return Err(format!("Unsupported expression: {text}"));
        }
        Ok(Self::Path(path))
    }

    fn resolve(&self, state: &State) -> Resolved {
        match self {
            Self::Literal(value) => Resolved {
                value: value.clone(),
                safe: value.is_string(),
                missing: false,
            },
            Self::Path(path) => {
                let value = state.lookup(path);
                Resolved {
                    missing: value.is_none(),
                    value: value.unwrap_or(Value::Null),
                    safe: is_super_path(path) && state.block_frames.last().is_some(),
                }
            }
        }
    }
}

struct Resolved {
    value: Value,
    safe: bool,
    missing: bool,
}

fn is_super_path(path: &[String]) -> bool {
    path == ["block", "super"]
}

impl Expression {
    fn needs_super(&self) -> bool {
        let needs = |atom: &Atom| matches!(atom, Atom::Path(path) if is_super_path(path));
        needs(&self.root)
            || self
                .filters
                .iter()
                .any(|(_, argument)| argument.as_ref().is_some_and(needs))
    }
    fn parse(text: &str) -> Result<Self> {
        let parts = split_quoted(text, |c| c == '|')?;
        if parts.len() > 65 {
            return Err("Filter chain is too complex".into());
        }
        let root = Atom::parse(&parts[0])?;
        let mut filters = Vec::new();
        for part in &parts[1..] {
            let pieces = split_quoted(part, |c| c == ':')?;
            let name = &pieces[0];
            let needs_argument = matches!(name.as_str(), "default" | "default_if_none" | "join");
            if !needs_argument
                && !matches!(
                    name.as_str(),
                    "upper" | "lower" | "length" | "escape" | "safe"
                )
            {
                return Err(format!("Unsupported filter: {name}"));
            }
            if pieces.len() != if needs_argument { 2 } else { 1 } {
                return Err(format!("Invalid argument count for filter: {name}"));
            }
            let argument = if needs_argument {
                Some(Atom::parse(&pieces[1])?)
            } else {
                None
            };
            filters.push((name.clone(), argument));
        }
        Ok(Self { root, filters })
    }

    fn resolve(&self, state: &State, ignore_failures: bool) -> Result<Resolved> {
        let mut resolved = self.root.resolve(state);
        if resolved.missing && !ignore_failures {
            resolved.value = Value::String(String::new());
        }
        for (filter, argument) in &self.filters {
            match filter.as_str() {
                "default" | "default_if_none" => {
                    if (filter == "default" && !truthy(&resolved.value))
                        || (filter == "default_if_none" && resolved.value.is_null())
                    {
                        resolved = argument
                            .as_ref()
                            .ok_or("Missing filter argument")?
                            .resolve(state);
                    }
                }
                "upper" => {
                    resolved.value = Value::String(display(&resolved.value)?.to_uppercase());
                    resolved.safe = false;
                }
                "lower" => {
                    resolved.value = Value::String(display(&resolved.value)?.to_lowercase());
                }
                "length" => {
                    let length = match &resolved.value {
                        Value::String(s) => s.chars().count(),
                        Value::Array(a) => a.len(),
                        Value::Object(o) => o.len(),
                        _ => 0,
                    };
                    resolved.value = json!(length);
                    resolved.safe = false;
                }
                "escape" => {
                    if !resolved.safe {
                        resolved.value = Value::String(escape(&display(&resolved.value)?));
                    }
                    resolved.safe = true;
                }
                "safe" => {
                    resolved.safe = true;
                }
                "join" => {
                    let separator = argument
                        .as_ref()
                        .ok_or("Missing join argument")?
                        .resolve(state);
                    let separator = if separator.safe {
                        display(&separator.value)?
                    } else {
                        escape(&display(&separator.value)?)
                    };
                    let Value::Array(values) = &resolved.value else {
                        return Err("Join requires a JSON array".into());
                    };
                    let strings = values
                        .iter()
                        .map(|value| display(value).map(|s| escape(&s)))
                        .collect::<Result<Vec<_>>>()?;
                    resolved.value = Value::String(strings.join(&separator));
                    resolved.safe = true;
                }
                _ => return Err(format!("Unsupported filter: {filter}")),
            }
            resolved.missing = false;
        }
        Ok(resolved)
    }
}

impl Condition {
    fn parse(text: &str) -> Result<Self> {
        let words = split_words(text)?;
        if words.len() > 64 {
            return Err("Condition is too complex".into());
        }
        Self::from_words(&words)
    }

    fn from_words(words: &[String]) -> Result<Self> {
        for operator in ["or", "and"] {
            if let Some(index) = words.iter().position(|word| word == operator) {
                let left = Box::new(Self::from_words(&words[..index])?);
                let right = Box::new(Self::from_words(&words[index + 1..])?);
                return Ok(if operator == "or" {
                    Self::Or(left, right)
                } else {
                    Self::And(left, right)
                });
            }
        }
        if words.first().is_some_and(|word| word == "not") {
            return Ok(Self::Not(Box::new(Self::from_words(&words[1..])?)));
        }
        match words {
            [expression] => Ok(Self::Value(Expression::parse(expression)?)),
            [left, operator, right]
                if matches!(
                    operator.as_str(),
                    "==" | "!=" | "<" | ">" | "<=" | ">=" | "in"
                ) =>
            {
                Ok(Self::Compare(
                    Expression::parse(left)?,
                    operator.clone(),
                    Expression::parse(right)?,
                ))
            }
            [left, not, operator, right] if not == "not" && operator == "in" => Ok(Self::Compare(
                Expression::parse(left)?,
                "not in".into(),
                Expression::parse(right)?,
            )),
            _ => Err("Unsupported if condition; use expressions separated by spaces".into()),
        }
    }

    fn evaluate(
        &self,
        engine: &Engine,
        blocks: &Blocks<'_>,
        state: &mut State,
        depth: usize,
    ) -> Result<bool> {
        match self {
            Self::Value(expression) => Ok(truthy(
                &engine
                    .resolve_expression(expression, blocks, state, depth, true)?
                    .value,
            )),
            Self::Not(condition) => Ok(!condition.evaluate(engine, blocks, state, depth)?),
            Self::And(left, right) => Ok(left.evaluate(engine, blocks, state, depth)?
                && right.evaluate(engine, blocks, state, depth)?),
            Self::Or(left, right) => Ok(left.evaluate(engine, blocks, state, depth)?
                || right.evaluate(engine, blocks, state, depth)?),
            Self::Compare(left, operator, right) => {
                let left = engine
                    .resolve_expression(left, blocks, state, depth, true)?
                    .value;
                let right = engine
                    .resolve_expression(right, blocks, state, depth, true)?
                    .value;
                let equal = left == right
                    || numeric(&left)
                        .zip(numeric(&right))
                        .is_some_and(|(a, b)| a == b);
                let order = match (&left, &right) {
                    (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
                    _ => numeric(&left)
                        .zip(numeric(&right))
                        .and_then(|(a, b)| a.partial_cmp(&b)),
                };
                let contains = || match (&left, &right) {
                    (Value::String(a), Value::String(b)) => b.contains(a),
                    (_, Value::Array(values)) => values.contains(&left),
                    (Value::String(key), Value::Object(values)) => values.contains_key(key),
                    _ => false,
                };
                Ok(match operator.as_str() {
                    "==" => equal,
                    "!=" => !equal,
                    "<" => order.is_some_and(|o| o.is_lt()),
                    ">" => order.is_some_and(|o| o.is_gt()),
                    "<=" => order.is_some_and(|o| o.is_le()),
                    ">=" => order.is_some_and(|o| o.is_ge()),
                    "in" => contains(),
                    "not in" => !contains(),
                    _ => false,
                })
            }
        }
    }
}

fn numeric(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(value) => Some(f64::from(u8::from(*value))),
        _ => value.as_f64(),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn display(value: &Value) -> Result<String> {
    match value {
        Value::Null => Ok("None".into()),
        Value::Bool(value) => Ok(if *value { "True" } else { "False" }.into()),
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err("Direct array/object output is unsupported; use a loop or filter".into()),
    }
}

fn escape(text: &str) -> String {
    let mut escaped = String::new();
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#x27;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

struct State {
    block_frames: Vec<BlockFrame>,
    origins: Vec<String>,
    scopes: Vec<Map<String, Value>>,
    output: String,
    steps: usize,
}

impl State {
    fn lookup(&self, path: &[String]) -> Option<Value> {
        if is_super_path(path)
            && let Some(frame) = self.block_frames.last()
        {
            return frame
                .super_output
                .as_ref()
                .map(|s| Value::String(s.clone()));
        }
        if path == ["block", "name"]
            && let Some(frame) = self.block_frames.last()
        {
            return Some(Value::String(frame.name.clone()));
        }
        let first = path.first()?;
        let mut value = self.scopes.iter().rev().find_map(|scope| scope.get(first));
        for part in &path[1..] {
            value = value.and_then(|value| match value {
                Value::Object(object) => object.get(part),
                Value::Array(array) => part
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| array.get(index)),
                _ => None,
            });
        }
        value.cloned()
    }

    fn step(&mut self) -> Result<()> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err("Render step limit exceeded".into());
        }
        Ok(())
    }

    fn append(&mut self, text: &str) -> Result<()> {
        if self.output.len() + text.len() > MAX_OUTPUT {
            return Err("Rendered output exceeds 1 MiB".into());
        }
        self.output.push_str(text);
        Ok(())
    }
}
