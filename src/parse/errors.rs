//! Errors raised while parsing a template.
use crate::path::RelativePathError;
use dtl_lexer::common::LexerError;
use dtl_lexer::tag::TagLexerError;
use dtl_lexer::tag::autoescape::AutoescapeError;
use dtl_lexer::tag::forloop::ForLexerError;
use dtl_lexer::tag::forloop::ForLexerInError;
use dtl_lexer::tag::include::IncludeLexerError;
use dtl_lexer::tag::kwarg::TagElementKwargLexerError;
use dtl_lexer::tag::lorem::LoremError;
use dtl_lexer::tag::now::NowError;
use dtl_lexer::tag::templatetag::TemplateTagError;
use dtl_lexer::variable::VariableLexerError;
use miette::Diagnostic;
use miette::SourceSpan;
use pyo3::exceptions::PyNotImplementedError;
use pyo3::prelude::*;
use std::borrow::Cow;
use thiserror::Error;

#[allow(clippy::enum_variant_names)]
#[derive(Error, Debug, Diagnostic, PartialEq, Eq)]
pub enum ForParseError {
    #[error("Expected another variable when unpacking in for loop:")]
    MissingVariable {
        #[label("after this variable")]
        at: SourceSpan,
    },
    #[error("Expected a variable name before the 'in' keyword:")]
    MissingVariableBeforeIn {
        #[label("before this keyword")]
        at: SourceSpan,
    },
    #[error("Expected at least one variable name in for loop:")]
    MissingVariableNames {
        #[label("in this tag")]
        at: SourceSpan,
    },
}

#[derive(Error, Debug, Diagnostic, PartialEq, Eq)]
pub enum ParseError {
    #[error("Empty variable tag")]
    EmptyVariable {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Expected an argument")]
    MissingArgument {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Expected two or more arguments")]
    MissingCycleArguments {
        #[label("expected at least two arguments")]
        at: SourceSpan,
    },
    #[error("Named cycle '{name}' does not exist")]
    #[diagnostic(help("Define the named cycle earlier using the 'as' form."))]
    UnknownNamedCycle {
        name: String,
        #[label("not defined")]
        at: SourceSpan,
    },
    #[error("Only 'silent' flag is allowed after cycle's name, not '{flag}'.")]
    InvalidCycleFlag {
        flag: String,
        #[label("invalid flag")]
        at: SourceSpan,
    },
    #[error(transparent)]
    #[diagnostic(transparent)]
    AutoescapeError(#[from] AutoescapeError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    BlockError(#[from] TagLexerError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    LexerError(#[from] LexerError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    ForLexerError(#[from] ForLexerError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    ForLexerInError(#[from] ForLexerInError),
    #[allow(clippy::enum_variant_names)]
    #[error(transparent)]
    #[diagnostic(transparent)]
    ForParseError(#[from] ForParseError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    IncludeLexerError(#[from] IncludeLexerError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    RelativePathError(#[from] RelativePathError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    TagElementKwargLexerError(#[from] TagElementKwargLexerError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    VariableError(#[from] VariableLexerError),
    #[error("The 'only' option was specified more than once.")]
    #[diagnostic(help("Remove the second 'only'"))]
    IncludeOnlyTwice {
        #[label("first here")]
        first_at: SourceSpan,
        #[label("second here")]
        second_at: SourceSpan,
    },
    #[error("Invalid filter: '{filter}'")]
    InvalidFilter {
        filter: String,
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Not expecting '{token}' in this position")]
    InvalidIfPosition {
        token: String,
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Invalid divisibility check: cannot divide by zero")]
    DivisibleByZero {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Invalid numeric literal")]
    InvalidNumber {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Missing boolean expression")]
    MissingBooleanExpression {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Unclosed '{start}' tag. Looking for one of: {expected}")]
    MissingEndTag {
        start: Cow<'static, str>,
        expected: Cow<'static, str>,
        #[label("started here")]
        at: SourceSpan,
    },
    #[error("'{tag}' is not a valid tag or filter in tag library '{library}'")]
    MissingFilterTag {
        tag: String,
        library: String,
        #[label("tag or filter")]
        tag_at: SourceSpan,
        #[label("library")]
        library_at: SourceSpan,
    },
    #[error("Expected a keyword argument")]
    MissingKeywordArgument {
        #[label("after this")]
        at: SourceSpan,
    },
    #[error("'{library}' is not a registered tag library.")]
    MissingTagLibrary {
        library: String,
        #[label("here")]
        at: SourceSpan,
        #[help]
        help: String,
    },
    #[error("Cannot mix positional and keyword arguments")]
    MixedArgsKwargs {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("'url' view name must be a string or variable, not a number")]
    NumericUrlName {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("'{name}' must have a first argument of 'content'")]
    RequiresContent {
        name: String,
        #[label("used here")]
        at: SourceSpan,
    },
    #[error(
        "'{name}' is decorated with takes_context=True so it must have a first argument of 'context'"
    )]
    RequiresContext {
        name: String,
        #[label("used here")]
        at: SourceSpan,
    },
    #[error(
        "'{name}' is decorated with takes_context=True so it must have a first argument of 'context' and a second argument of 'content'"
    )]
    RequiresContextAndContent {
        name: String,
        #[label("used here")]
        at: SourceSpan,
    },
    #[error("'{tag_name}' did not receive value(s) for the argument(s): {missing}")]
    MissingArguments {
        tag_name: String,
        missing: String,
        #[label("here")]
        at: SourceSpan,
    },
    #[error("'{tag_name}' received multiple values for keyword argument '{kwarg_name}'")]
    DuplicateKeywordArgument {
        tag_name: String,
        kwarg_name: String,
        #[label("first")]
        first_at: SourceSpan,
        #[label("second")]
        second_at: SourceSpan,
    },
    #[error("Unexpected positional argument after keyword argument")]
    PositionalAfterKeyword {
        #[label("this positional argument")]
        at: SourceSpan,
        #[label("after this keyword argument")]
        after: SourceSpan,
    },
    #[error("Unexpected positional argument")]
    TooManyPositionalArguments {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Unexpected keyword argument")]
    UnexpectedKeywordArgument {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("{filter} filter does not take an argument")]
    UnexpectedArgument {
        filter: &'static str,
        #[label("unexpected argument")]
        at: SourceSpan,
    },
    #[error("Unexpected end of expression")]
    UnexpectedEndExpression {
        #[label("after this")]
        at: SourceSpan,
    },
    #[error("Unexpected tag {unexpected}")]
    UnexpectedEndTag {
        unexpected: Cow<'static, str>,
        #[label("unexpected tag")]
        at: SourceSpan,
    },
    #[error("Unused expression '{expression}' in if tag")]
    UnusedExpression {
        expression: String,
        #[label("here")]
        at: SourceSpan,
    },
    #[error("'url' takes at least one argument, a URL pattern name")]
    UrlTagNoArguments {
        #[label("here")]
        at: SourceSpan,
    },
    #[error("Unexpected tag {unexpected}, expected {expected}")]
    WrongEndTag {
        unexpected: Cow<'static, str>,
        expected: String,
        #[label("unexpected tag")]
        at: SourceSpan,
        #[label("start tag")]
        start_at: SourceSpan,
    },
    #[error("Incorrect format for '{tag}' tag")]
    InvalidTagFormat {
        tag: &'static str,
        #[label("here")]
        at: SourceSpan,
    },

    #[error("Invalid variable name")]
    InvalidVariableName {
        #[label("here")]
        at: SourceSpan,
    },

    #[error(transparent)]
    #[diagnostic(transparent)]
    LoremError(#[from] LoremError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    NowError(#[from] NowError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    TemplateTagError(#[from] TemplateTagError),

    #[error("Expected a variable name after 'as'")]
    #[diagnostic(help("Provide a name to store the date string, e.g. 'as my_var'"))]
    MissingVariableAfterAs {
        #[label("expected a variable name here")]
        at: SourceSpan,
    },

    #[error("Unexpected tokens after 'as {var_name}'")]
    #[diagnostic(help("Remove the extra tokens."))]
    UnexpectedTokensAfterAsVariable {
        var_name: String,
        #[label("unexpected tokens here")]
        at: SourceSpan,
    },
}

/// Template syntax the native parser does not support. Python sees it as
/// `NotImplementedError`.
#[derive(Error, Debug)]
pub enum Unimplemented {
    /// A tag with no native parser.
    #[error("not yet implemented: {0}")]
    Tag(String),
    /// A loaded tag whose compiler needs Django's full parser.
    #[error("not yet implemented: Fully custom tag")]
    FullyCustomTag,
}

/// What happens to a template the native parser cannot compile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// Django's parser compiles it, and Rust renders what it can.
    Django,
    /// No parser can compile it.
    Unsupported,
}

impl Unimplemented {
    pub fn fallback(&self) -> Fallback {
        match self {
            // Partials only parse natively from Django 6.0.
            Self::Tag(name) if matches!(name.as_str(), "partialdef" | "partial") => {
                Fallback::Django
            }
            Self::Tag(_) => Fallback::Unsupported,
            Self::FullyCustomTag => Fallback::Django,
        }
    }
}

impl From<Unimplemented> for PyErr {
    fn from(error: Unimplemented) -> Self {
        PyNotImplementedError::new_err(error.to_string())
    }
}

#[derive(Error, Debug)]
pub enum PyParseError {
    #[error(transparent)]
    PyErr(#[from] PyErr),
    #[error(transparent)]
    ParseError(#[from] ParseError),
    #[error(transparent)]
    Unimplemented(#[from] Unimplemented),
}

impl PyParseError {
    pub fn try_into_parse_error(self) -> PyResult<ParseError> {
        match self {
            Self::ParseError(err) => Ok(err),
            Self::PyErr(err) => Err(err),
            Self::Unimplemented(err) => Err(err.into()),
        }
    }

    #[cfg(test)]
    pub fn unwrap_parse_error(self) -> ParseError {
        match self {
            Self::ParseError(err) => err,
            Self::PyErr(err) => panic!("{err:?}"),
            Self::Unimplemented(err) => panic!("{err:?}"),
        }
    }
}
