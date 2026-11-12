//! Node types that make up the parsed `TokenTree`.
use crate::filters::FilterType;
use crate::gc::Gc;
use crate::parse::Filter;
use crate::parse::IfCondition;
use crate::parse::StockNode;
use crate::template::django_rusty_templates::Engine;
use crate::types::ArgumentType;
use crate::types::ForVariable;
use crate::types::Text;
use dtl_lexer::tag::autoescape::AutoescapeEnabled;
use dtl_lexer::tag::lorem::LoremMethod;
use dtl_lexer::tag::templatetag::TemplateTag;
use dtl_lexer::types::At;
use dtl_lexer::types::Variable;
use either::Either;
use num_bigint::BigInt;
use pyo3::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub struct Lorem {
    pub count: TagElement,
    pub method: LoremMethod,
    pub common: bool,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Comment;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CsrfToken;

#[derive(Clone, Debug)]
pub struct InclusionTag {
    pub id: CycleId,
    pub call: SimpleTag,
    pub filename: Gc<Py<PyAny>>,
    pub engine: Gc<Engine>,
    /// Django admin's `InclusionAdminNode` template name. Its template is
    /// chosen per render from `opts`, and its function sees the live context.
    pub admin_template: Option<&'static str>,
}

impl PartialEq for InclusionTag {
    fn eq(&self, other: &Self) -> bool {
        self.call == other.call
            && self.admin_template == other.admin_template
            && Gc::ptr_eq(&self.filename, &other.filename)
            && Gc::ptr_eq(&self.engine, &other.engine)
    }
}

#[derive(Clone, Debug)]
pub struct Regroup {
    pub target: TagElement,
    pub expression: Gc<Py<PyAny>>,
    pub asvar: String,
    /// The attribute path of a plain `by` key, such as `["country", "name"]`.
    /// Keys with filters or literals resolve through `expression`.
    pub lookup: Option<Vec<String>>,
}

impl PartialEq for Regroup {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target
            && self.asvar == other.asvar
            && self.lookup == other.lookup
            && Gc::ptr_eq(&self.expression, &other.expression)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TagElement {
    Int(BigInt),
    Float(f64),
    Text(Text),
    TranslatedText(Text),
    Variable(Variable),
    ForVariable(ForVariable),
    Filter(Box<Filter>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Url {
    pub at: At,
    pub view_name: TagElement,
    pub args: Vec<TagElement>,
    pub kwargs: Vec<(String, TagElement)>,
    pub asvar: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForIterable {
    pub iterable: TagElement,
    pub at: At,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForNames {
    pub names: Vec<String>,
    pub at: At,
}

#[derive(Clone, Debug, PartialEq)]
pub struct For {
    pub iterable: ForIterable,
    pub variables: ForNames,
    pub reversed: bool,
    pub body: Vec<TokenTree>,
    pub empty: Option<Vec<TokenTree>>,
    /// No node in the body or empty branch can see a Django `Context`, so a
    /// live render can run this loop on native scopes. See `callback_free`.
    pub callback_free: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelativePath {
    pub at: At,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IncludeTemplateName {
    Text(Text),
    Variable(TagElement),
    Relative(RelativePath),
}

#[derive(Clone, Debug)]
pub struct Include {
    pub id: CycleId,
    pub template_name: IncludeTemplateName,
    pub origin: Option<String>,
    pub engine: Gc<Engine>,
    pub only: bool,
    pub kwargs: Vec<(At, TagElement)>,
}

impl PartialEq for Include {
    fn eq(&self, other: &Self) -> bool {
        // We use `Gc::ptr_eq` here to avoid needing the `py` token for true
        // equality comparison between two `Py` smart pointers.
        //
        // We only use `eq` in tests, so this concession is acceptable here.
        self.only == other.only
            && self.origin == other.origin
            && self.template_name.eq(&other.template_name)
            && self.kwargs == other.kwargs
            && Gc::ptr_eq(&self.engine, &other.engine)
    }
}

#[derive(Clone, Debug)]
pub struct SimpleTag {
    pub func: Gc<Py<PyAny>>,
    pub at: At,
    pub takes_context: bool,
    pub args: Vec<TagElement>,
    pub kwargs: Vec<(String, TagElement)>,
    pub target_var: Option<String>,
}

impl PartialEq for SimpleTag {
    fn eq(&self, other: &Self) -> bool {
        // We use `Gc::ptr_eq` here to avoid needing the `py` token for true
        // equality comparison between two `Py` smart pointers.
        //
        // We only use `eq` in tests, so this concession is acceptable here.
        self.at == other.at
            && self.takes_context == other.takes_context
            && self.args == other.args
            && self.kwargs == other.kwargs
            && self.target_var == other.target_var
            && Gc::ptr_eq(&self.func, &other.func)
    }
}

#[derive(Clone, Debug)]
pub struct SimpleBlockTag {
    pub func: Gc<Py<PyAny>>,
    pub nodes: Vec<TokenTree>,
    pub at: At,
    pub takes_context: bool,
    pub args: Vec<TagElement>,
    pub kwargs: Vec<(String, TagElement)>,
    pub target_var: Option<String>,
}

impl PartialEq for SimpleBlockTag {
    fn eq(&self, other: &Self) -> bool {
        // We use `Gc::ptr_eq` here to avoid needing the `py` token for true
        // equality comparison between two `Py` smart pointers.
        //
        // We only use `eq` in tests, so this concession is acceptable here.
        self.at == other.at
            && self.takes_context == other.takes_context
            && self.args == other.args
            && self.kwargs == other.kwargs
            && self.target_var == other.target_var
            && self.nodes == other.nodes
            && Gc::ptr_eq(&self.func, &other.func)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartialDefinition {
    pub name: String,
    pub nodes: Gc<Vec<TokenTree>>,
    pub span: At,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Now {
    pub format: String,
    pub expression: Option<TagElement>,
    pub escape: bool,
    pub asvar: Option<At>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FirstOf {
    pub vars: Vec<TagElement>,
    pub asvar: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CycleValue {
    pub value: TagElement,
    pub at: At,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimpleCycle {
    pub id: CycleId,
    pub values: Vec<CycleValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NamedCycle {
    pub cycle: SimpleCycle,
    pub asvar: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SilentNamedCycle {
    pub cycle: NamedCycle,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cycle {
    Simple(SimpleCycle),
    Named(NamedCycle),
    SilentNamed(SilentNamedCycle),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CycleId(pub(crate) usize);

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Stock {
        node: Box<StockNode>,
        at: At,
    },
    PartialDef {
        definition: PartialDefinition,
        inline: bool,
    },
    Partial {
        name: String,
        at: At,
    },
    PythonNode(PythonNode),
    Block {
        name: String,
        nodes: Gc<Vec<TokenTree>>,
    },
    Extends {
        at: At,
        parent: TagElement,
        parent_token: String,
        relative: Option<String>,
        nodes: Vec<TokenTree>,
    },
    Autoescape {
        enabled: AutoescapeEnabled,
        nodes: Vec<TokenTree>,
    },
    If {
        condition: IfCondition,
        truthy: Vec<TokenTree>,
        falsey: Option<Vec<TokenTree>>,
    },
    IfChanged {
        id: CycleId,
        values: Vec<TagElement>,
        truthy: Vec<TokenTree>,
        falsey: Vec<TokenTree>,
    },
    Regroup(Regroup),
    For(For),
    Include(Include),
    Load,
    ResetCycle(CycleId),
    With {
        assignments: Vec<(String, TagElement)>,
        nodes: Vec<TokenTree>,
    },
    FilterBlock {
        filters: Vec<FilterType>,
        nodes: Vec<TokenTree>,
    },
    Spaceless(Vec<TokenTree>),
    Verbatim(Vec<TokenTree>),
    Debug,
    QueryString(SimpleTag),
    InclusionTag(InclusionTag),
    SimpleTag(SimpleTag),
    SimpleBlockTag(SimpleBlockTag),
    Url(Url),
    CsrfToken(CsrfToken),
    Lorem(Lorem),
    Comment(Comment),
    Now(Now),
    FirstOf(FirstOf),
    WidthRatio {
        args: Vec<TagElement>,
        asvar: Option<String>,
    },
    TemplateTag(TemplateTag),
    Cycle(Cycle),
}

#[derive(Clone, Debug)]
pub struct PythonNode(pub Gc<Py<PyAny>>);

impl PartialEq for PythonNode {
    fn eq(&self, other: &Self) -> bool {
        Gc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenTree {
    Text(Text),
    LiteralText(Text),
    TranslatedText(Text),
    Int(BigInt),
    Float(f64),
    Tag(Box<Tag>),
    Variable(Variable),
    ForVariable(ForVariable),
    Filter(Box<Filter>),
}

impl TokenTree {
    /// Tags are boxed so a text or variable node, which is most of a
    /// template, is not as large as the largest tag.
    pub fn tag(tag: Tag) -> Self {
        Self::Tag(Box::new(tag))
    }

    pub fn as_tag(&self) -> Option<&Tag> {
        match self {
            Self::Tag(tag) => Some(tag),
            _ => None,
        }
    }

    pub fn into_tag(self) -> Option<Tag> {
        match self {
            Self::Tag(tag) => Some(*tag),
            _ => None,
        }
    }

    /// Child lists used to discover inheritance blocks without a Python tree.
    pub fn child_nodes(&self) -> Vec<&[Self]> {
        let Some(tag) = self.as_tag() else {
            return Vec::new();
        };
        match tag {
            Tag::Stock { node, .. } => node.child_nodes(),
            Tag::Block { nodes, .. } => vec![nodes],
            Tag::PartialDef { definition, .. } => vec![&definition.nodes],
            Tag::Extends { nodes, .. }
            | Tag::Autoescape { nodes, .. }
            | Tag::With { nodes, .. }
            | Tag::FilterBlock { nodes, .. }
            | Tag::Spaceless(nodes)
            | Tag::Verbatim(nodes) => vec![nodes],
            Tag::If { truthy, falsey, .. } => {
                vec![truthy, falsey.as_deref().unwrap_or_default()]
            }
            Tag::IfChanged { truthy, falsey, .. } => vec![truthy, falsey],
            Tag::For(node) => {
                vec![&node.body, node.empty.as_deref().unwrap_or_default()]
            }
            Tag::SimpleBlockTag(node) => vec![&node.nodes],
            _ => Vec::new(),
        }
    }
}

impl From<TagElement> for TokenTree {
    fn from(tag_element: TagElement) -> Self {
        match tag_element {
            TagElement::Text(text) => Self::LiteralText(text),
            TagElement::TranslatedText(text) => Self::TranslatedText(text),
            TagElement::Variable(variable) => Self::Variable(variable),
            TagElement::ForVariable(variable) => Self::ForVariable(variable),
            TagElement::Filter(filter) => Self::Filter(filter),
            TagElement::Int(n) => Self::Int(n),
            TagElement::Float(f) => Self::Float(f),
        }
    }
}

impl From<Either<Variable, ForVariable>> for TagElement {
    fn from(variable: Either<Variable, ForVariable>) -> Self {
        match variable {
            Either::Left(v) => Self::Variable(v),
            Either::Right(v) => Self::ForVariable(v),
        }
    }
}

impl From<Either<Variable, ForVariable>> for ArgumentType {
    fn from(variable: Either<Variable, ForVariable>) -> Self {
        match variable {
            Either::Left(v) => Self::Variable(v),
            Either::Right(v) => Self::ForVariable(v),
        }
    }
}
