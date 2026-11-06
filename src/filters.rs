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
    First(FirstFilter),
    FloatFormat(FloatFormatFilter),
    ForceEscape(ForceEscapeFilter),
    Join(JoinFilter),
    Justify(JustifyFilter),
    Last(LastFilter),
    Linebreaks(LinebreaksFilter),
    Linebreaksbr(LinebreaksbrFilter),
    Linenumbers(LinenumbersFilter),
    Lower(LowerFilter),
    Length(LengthFilter),
    Safe(SafeFilter),
    Slice(SliceFilter),
    Slugify(SlugifyFilter),
    Timesince(TimesinceFilter),
    Title(TitleFilter),
    TruncateChars(TruncateCharsFilter),
    TruncateCharsHtml(TruncateCharsHtmlFilter),
    TruncateWords(TruncateWordsFilter),
    Pluralize(PluralizeFilter),
    GetDigit(GetDigitFilter),
    Phone2numeric(Phone2numericFilter),
    Urlencode(UrlencodeFilter),
    Iriencode(IriencodeFilter),
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
pub struct FirstFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForceEscapeFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LengthFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinebreaksFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinebreaksbrFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinenumbersFilter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeFilter;

/// Django's `slice`, computed natively for strings and Python objects. A
/// literal argument is parsed once when the template is compiled.
#[derive(Clone, Debug, PartialEq)]
pub struct SliceFilter {
    pub django: DjangoFilter,
    /// The parsed literal argument, or `None` to resolve it at render.
    pub literal: Option<SliceArgument>,
}

/// The `slice(*bits)` Django builds from the filter argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceArgument {
    Bounds {
        start: Option<isize>,
        stop: Option<isize>,
        step: Option<isize>,
    },
    /// An argument Django rejects, so the value is returned unchanged.
    Invalid,
}

impl SliceArgument {
    /// `slice(n)` for an integer argument, which `str()` turns into one bit.
    pub fn stop(stop: isize) -> Self {
        Self::Bounds {
            start: None,
            stop: Some(stop),
            step: None,
        }
    }

    /// Parses `str(arg)` the way Django does. Returns `None` for bits that
    /// are not plain optionally signed ASCII integers, leaving Python's
    /// `int()` rules (whitespace, underscores, Unicode digits) to Django.
    pub fn parse(text: &str) -> Option<Self> {
        let bits: Vec<&str> = text.split(':').collect();
        // `slice()` takes at most three arguments, so Django's TypeError
        // returns the value whatever the bits are.
        if bits.len() > 3 {
            return Some(Self::Invalid);
        }
        let mut values = [None; 3];
        for (value, bit) in values.iter_mut().zip(&bits) {
            if bit.is_empty() {
                continue;
            }
            let digits = bit.strip_prefix(['-', '+']).unwrap_or(bit);
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            *value = Some(bit.parse().ok()?);
        }
        let [first, second, step] = values;
        Some(match bits.len() {
            1 => Self::Bounds {
                start: None,
                stop: first,
                step: None,
            },
            _ => Self::Bounds {
                start: first,
                stop: second,
                step,
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlugifyFilter;

/// Django's `timesince` or `timeuntil`, computed natively for `date` and
/// `datetime` values. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct TimesinceFilter {
    pub django: DjangoFilter,
    /// `timeuntil`, which measures from the comparison time to the value.
    pub until: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleFilter;

/// Django's `truncatechars`, computed natively for ASCII strings and an
/// integer length. Other strings call Django's `Truncator` with the cached
/// translation of the truncation text, and other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct TruncateCharsFilter {
    pub django: DjangoFilter,
}

/// Django's `truncatechars_html`. Strings with an integer length call
/// Django's HTML `Truncator` with the cached translation of the truncation
/// text; other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct TruncateCharsHtmlFilter {
    pub django: DjangoFilter,
}

/// Django's `ljust` and `rjust`, computed natively for strings and an
/// integer width. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct JustifyFilter {
    pub django: DjangoFilter,
    pub side: Justify,
}

/// Which side `ljust` and `rjust` keep the text on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    Left,
    Right,
}

/// Django's `truncatewords`, computed natively for strings and an integer
/// length. Other inputs call Django's filter.
/// Django's `pluralize`, computed natively for numbers, bools, lists, tuples
/// and dicts with a string suffix argument. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct PluralizeFilter {
    pub django: DjangoFilter,
}

/// Django's `get_digit`, computed natively for non-negative integers and
/// digit strings with an integer argument. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct GetDigitFilter {
    pub django: DjangoFilter,
}

/// Django's `phone2numeric`, computed natively for ASCII strings. Other
/// inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct Phone2numericFilter {
    pub django: DjangoFilter,
}

/// Django's `urlencode`, computed natively with the default or an ASCII
/// `safe` argument. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct UrlencodeFilter {
    pub django: DjangoFilter,
}

/// Django's `iriencode`, computed natively. Values whose `str()` fails call
/// Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct IriencodeFilter {
    pub django: DjangoFilter,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TruncateWordsFilter {
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

/// Django's `join`, computed natively for lists and tuples and a string
/// separator. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct JoinFilter {
    pub django: DjangoFilter,
}
