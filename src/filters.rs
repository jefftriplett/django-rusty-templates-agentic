use crate::gc::Gc;

use pyo3::prelude::*;

use crate::types::Argument;
use dtl_lexer::types::At;

#[derive(Clone, Debug, PartialEq)]
pub enum FilterType {
    Add(AddFilter),
    AddSlashes(AddSlashesFilter),
    Capfirst(CapfirstFilter),
    Center(CenterFilter),
    Cut(CutFilter),
    Default(DefaultFilter),
    DefaultIfNone(DefaultIfNoneFilter),
    DivisibleBy(DivisibleByFilter),
    Date(DateFilter),
    Django(DjangoFilter),
    Escape(EscapeFilter),
    Escapejs(EscapejsFilter),
    External(ExternalFilter),
    FloatFormat(FloatFormatFilter),
    ForceEscape(ForceEscapeFilter),
    Last(LastFilter),
    Lower(LowerFilter),
    Length(LengthFilter),
    Safe(SafeFilter),
    Slugify(SlugifyFilter),
    Title(TitleFilter),
    TruncateChars(TruncateCharsFilter),
    Upper(UpperFilter),
    Wordcount(WordcountFilter),
    Wordwrap(WordwrapFilter),
    Yesno(YesnoFilter),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddSlashesFilter;

#[derive(Clone, Debug, PartialEq)]
pub struct AddFilter {
    pub argument: Argument,
}

impl AddFilter {
    pub fn new(argument: Argument) -> Self {
        Self { argument }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapfirstFilter;

#[derive(Clone, Debug, PartialEq)]
pub struct CenterFilter {
    pub argument: Argument,
}

impl CenterFilter {
    pub fn new(argument: Argument) -> Self {
        Self { argument }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CutFilter {
    pub argument: Argument,
}

impl CutFilter {
    pub fn new(argument: Argument) -> Self {
        Self { argument }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DefaultFilter {
    pub argument: Argument,
    pub at: At,
}

impl DefaultFilter {
    pub fn new(argument: Argument, at: At) -> Self {
        Self { argument, at }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DefaultIfNoneFilter {
    pub argument: Argument,
}

impl DefaultIfNoneFilter {
    pub fn new(argument: Argument) -> Self {
        Self { argument }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DateFilter {
    pub argument: Option<Argument>,
    pub at: At,
}

impl DateFilter {
    pub fn new(argument: Option<Argument>, at: At) -> Self {
        Self { argument, at }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscapeFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscapejsFilter;

#[derive(Clone, Debug)]
pub struct ExternalFilter {
    pub filter: Gc<Py<PyAny>>,
    pub argument: Option<Argument>,
}

impl ExternalFilter {
    pub fn new(filter: Py<PyAny>, argument: Option<Argument>) -> Self {
        Self {
            filter: Gc::new(filter),
            argument,
        }
    }
}

impl PartialEq for ExternalFilter {
    fn eq(&self, other: &Self) -> bool {
        // We use `Gc::ptr_eq` here to avoid needing the `py` token for true
        // equality comparison between two `Py` smart pointers.
        //
        // We only use `eq` in tests, so this concession is acceptable here.
        self.argument.eq(&other.argument) && Gc::ptr_eq(&self.filter, &other.filter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastFilter {
    pub at: (usize, usize),
}

impl LastFilter {
    pub fn new(at: (usize, usize)) -> Self {
        Self { at }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForceEscapeFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LengthFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlugifyFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleFilter;

/// Django's `truncatechars`, computed natively for ASCII strings and an
/// integer length. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct TruncateCharsFilter {
    pub django: DjangoFilter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpperFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordcountFilter;

#[derive(Clone, Debug, PartialEq)]
pub struct WordwrapFilter {
    pub argument: Argument,
}

impl WordwrapFilter {
    pub fn new(argument: Argument) -> Self {
        Self { argument }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct YesnoFilter {
    pub at: At,
    pub argument: Option<Argument>,
}

impl YesnoFilter {
    pub fn new(at: At, argument: Option<Argument>) -> Self {
        Self { at, argument }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DivisibleByFilter {
    pub at: At,
    pub argument: Argument,
}

impl DivisibleByFilter {
    pub fn new(at: At, argument: Argument) -> Self {
        Self { at, argument }
    }
}

/// Django-backed filters preserve Python protocol and escaping behavior.
#[derive(Clone, Debug)]
pub struct DjangoFilter {
    pub filter: Gc<Py<PyAny>>,
    pub argument: Option<Argument>,
}

impl PartialEq for DjangoFilter {
    fn eq(&self, other: &Self) -> bool {
        self.argument == other.argument && self.filter.as_ptr() == other.filter.as_ptr()
    }
}

/// Django's `floatformat`, computed natively for `int`, `float` and `Decimal`
/// values and an integer precision. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct FloatFormatFilter {
    pub django: DjangoFilter,
}
