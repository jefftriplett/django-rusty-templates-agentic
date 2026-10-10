//! A deliberately limited, Python-free DTL runtime for browser experiments.
//! The production Django backend does not use this crate.

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
}

/// Templates are compiled on registration and reused across renders.
#[derive(Default)]
pub struct Engine {
    templates: HashMap<String, Vec<Node>>,
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
        self.templates.insert(name.into(), nodes);
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
        };
        self.render_template(name, &mut state, 0)?;
        Ok(state.output)
    }

    fn render_template(&self, name: &str, state: &mut State, depth: usize) -> Result<()> {
        let nodes = self
            .templates
            .get(name)
            .ok_or_else(|| format!("Template not found: {name}"))?;
        self.render_nodes(nodes, state, depth)
    }

    fn render_nodes(&self, nodes: &[Node], state: &mut State, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err("Template nesting/include depth exceeded".into());
        }
        for node in nodes {
            state.step()?;
            match node {
                Node::Text(text) => state.append(text)?,
                Node::Output(expression) => {
                    let resolved = expression.resolve(state, false)?;
                    let text = if resolved.missing {
                        String::new()
                    } else {
                        display(&resolved.value)?
                    };
                    state.append(&if resolved.safe { text } else { escape(&text) })?;
                }
                Node::Include(expression) => {
                    let value = expression.resolve(state, false)?.value;
                    let name = value.as_str().ok_or("Include name must be a string")?;
                    self.render_template(name, state, depth + 1)?;
                }
                Node::If(branches, otherwise) => {
                    let mut selected = otherwise;
                    for (condition, body) in branches {
                        if condition.evaluate(state)? {
                            selected = body;
                            break;
                        }
                    }
                    self.render_nodes(selected, state, depth + 1)?;
                }
                Node::For(name, expression, reversed, body, empty) => {
                    let value = expression.resolve(state, true)?.value;
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
                        self.render_nodes(empty, state, depth + 1)?;
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
                        let result = self.render_nodes(body, state, depth + 1);
                        state.scopes.pop();
                        result?;
                    }
                }
            }
        }
        Ok(())
    }
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
                    safe: false,
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

impl Expression {
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

    fn evaluate(&self, state: &State) -> Result<bool> {
        match self {
            Self::Value(expression) => Ok(truthy(&expression.resolve(state, true)?.value)),
            Self::Not(condition) => Ok(!condition.evaluate(state)?),
            Self::And(left, right) => Ok(left.evaluate(state)? && right.evaluate(state)?),
            Self::Or(left, right) => Ok(left.evaluate(state)? || right.evaluate(state)?),
            Self::Compare(left, operator, right) => {
                let left = left.resolve(state, true)?.value;
                let right = right.resolve(state, true)?.value;
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
    scopes: Vec<Map<String, Value>>,
    output: String,
    steps: usize,
}

impl State {
    fn lookup(&self, path: &[String]) -> Option<Value> {
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

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub struct BrowserEngine {
        inner: super::Engine,
    }

    #[wasm_bindgen]
    impl BrowserEngine {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self {
                inner: super::Engine::new(),
            }
        }

        #[wasm_bindgen(js_name = addTemplate)]
        pub fn add_template(
            &mut self,
            name: &str,
            source: &str,
        ) -> std::result::Result<(), JsValue> {
            self.inner
                .add_template(name, source)
                .map_err(|error| js_sys_error(&error))
        }

        pub fn render(
            &self,
            name: &str,
            context_json: &str,
        ) -> std::result::Result<String, JsValue> {
            self.inner
                .render(name, context_json)
                .map_err(|error| js_sys_error(&error))
        }
    }

    fn js_sys_error(error: &str) -> JsValue {
        JsValue::from_str(error)
    }
}
