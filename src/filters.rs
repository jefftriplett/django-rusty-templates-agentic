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
    Tz(TzFilter),
    L10n(L10nFilter),
    LanguageInfo(LanguageInfoFilter),
    Humanize(HumanizeFilter),
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
    MakeList(MakeListFilter),
    Escapeseq(EscapeseqFilter),
    Safeseq(SafeseqFilter),
    Random(RandomFilter),
    Pprint(PprintFilter),
    Filesizeformat(FilesizeformatFilter),
    UnorderedList(UnorderedListFilter),
    Striptags(StriptagsFilter),
    Stringformat(StringformatFilter),
    Dictsort(DictsortFilter),
    Dictsortreversed(DictsortreversedFilter),
    JsonScript(JsonScriptFilter),
    TruncateWordsHtml(TruncateWordsHtmlFilter),
    Urlize(UrlizeFilter),
    Urlizetrunc(UrlizetruncFilter),
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
    /// [`python_int`] can't convert, leaving them to Django.
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
            *value = Some(isize::try_from(python_int(bit)?).ok()?);
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

/// Python's `int(text)` for ASCII text: surrounding whitespace, an optional
/// sign, and digits with single underscores between them. Other text,
/// including non-ASCII digits and whitespace, returns `None` and is left to
/// Python, as are values beyond `i64`.
pub fn python_int(text: &str) -> Option<i64> {
    // The ASCII whitespace `int()` strips; unlike `str.isspace()`, it keeps
    // the separators U+001C to U+001F.
    let text = text.trim_matches(|c: char| matches!(c, '\t'..='\r' | ' '));
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    let mut previous_digit = false;
    for byte in digits.bytes() {
        match byte {
            b'0'..=b'9' => previous_digit = true,
            b'_' if previous_digit => previous_digit = false,
            _ => return None,
        }
    }
    if !previous_digit {
        return None;
    }
    let negative = text.starts_with('-');
    let mut value: i64 = 0;
    for byte in digits.bytes().filter(u8::is_ascii_digit) {
        let digit = i64::from(byte - b'0');
        value = value
            .checked_mul(10)?
            .checked_add(if negative { -digit } else { digit })?;
    }
    Some(value)
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

/// Django's `make_list`, building the list of characters natively.
#[derive(Clone, Debug, PartialEq)]
pub struct MakeListFilter {
    pub django: DjangoFilter,
}

/// Django's `escapeseq`, walking lists and tuples natively. Other values
/// call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct EscapeseqFilter {
    pub django: DjangoFilter,
}

/// Django's `safeseq`, walking lists and tuples natively. Other values call
/// Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct SafeseqFilter {
    pub django: DjangoFilter,
}

/// Django's `random`, calling `random.choice` directly for lists and tuples.
/// Other values call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct RandomFilter {
    pub django: DjangoFilter,
}

/// Django's `pprint`, calling `pprint.pformat` directly for values that
/// aren't safe strings. Safe strings call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct PprintFilter {
    pub django: DjangoFilter,
}

/// Django's `filesizeformat`, computed natively for values `int()` converts,
/// below 2⁵³. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct FilesizeformatFilter {
    pub django: DjangoFilter,
}

/// Django's `unordered_list`, built natively for nested lists and tuples.
/// Other values call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct UnorderedListFilter {
    pub django: DjangoFilter,
}

/// Django's `striptags`, done natively for well-formed tags.
/// Other text calls Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct StriptagsFilter {
    pub django: DjangoFilter,
}

/// Django's `stringformat`, done natively for common specifiers on strings,
/// integers and floats. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct StringformatFilter {
    pub django: DjangoFilter,
}

/// Django's `dictsort`, sorting lists and tuples natively by a key path.
/// Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct DictsortFilter {
    pub django: DjangoFilter,
}

/// Django's `dictsortreversed`, sorting lists and tuples natively by a key
/// path. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct DictsortreversedFilter {
    pub django: DjangoFilter,
}

/// Django's `json_script`, serialized natively for plain JSON values.
/// Other values call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonScriptFilter {
    pub django: DjangoFilter,
}

/// Django's `truncatewords_html`, truncated natively for well-formed HTML.
/// Other text calls Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct TruncateWordsHtmlFilter {
    pub django: DjangoFilter,
}

/// Django's `urlize`. Links are built by `render::urlize`, which leaves a
/// few kinds of word to Django's `Urlizer`.
#[derive(Clone, Debug, PartialEq)]
pub struct UrlizeFilter {
    pub django: DjangoFilter,
}

/// Django's `urlizetrunc`. Links are built by `render::urlize`, which leaves
/// a few kinds of word to Django's `Urlizer`.
#[derive(Clone, Debug, PartialEq)]
pub struct UrlizetruncFilter {
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

/// Which conversion one of the `{% load tz %}` filters makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TzConversion {
    /// `localtime`: to the current time zone.
    Localtime,
    /// `utc`: to UTC.
    Utc,
    /// `timezone:arg`: to a named time zone or tzinfo.
    Timezone,
}

/// Django's `localtime`, `utc` and `timezone` filters, converted natively for
/// exact `datetime` values. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct TzFilter {
    pub conversion: TzConversion,
    pub django: DjangoFilter,
}

/// Django's `localize` and `unlocalize` filters, formatted natively for
/// numbers, dates and strings. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct L10nFilter {
    /// `true` for `localize`, `false` for `unlocalize`.
    pub localize: bool,
    pub django: DjangoFilter,
}

/// Which field of `get_language_info(code)` an `{% load i18n %}` filter reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageField {
    /// `language_name`.
    Name,
    /// `language_name_local`.
    NameLocal,
    /// `language_name_translated`: `gettext` of the English name.
    NameTranslated,
    /// `language_bidi`.
    Bidi,
}

/// Django's `language_name*` and `language_bidi` filters, answered from a
/// per-render cache of `get_language_info`. Other inputs call Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct LanguageInfoFilter {
    pub field: LanguageField,
    pub django: DjangoFilter,
}

/// Which `{% load humanize %}` filter a [`HumanizeFilter`] stands in for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HumanizeKind {
    Apnumber,
    Intcomma,
    Intword,
    Naturalday,
    Naturaltime,
    Ordinal,
}

/// Django's `humanize` filters, computed natively for plain numbers and
/// datetimes with translations cached per language. Other inputs call
/// Django's filter.
#[derive(Clone, Debug, PartialEq)]
pub struct HumanizeFilter {
    pub kind: HumanizeKind,
    pub django: DjangoFilter,
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
