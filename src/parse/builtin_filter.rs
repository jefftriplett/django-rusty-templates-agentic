//! Builtin Django filters that are resolved from Django's filter library
//! rather than reimplemented natively.

/// Whether a builtin filter takes an argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FilterArgument {
    Forbidden,
    Optional,
    Required,
}

/// Declares `BuiltinFilter` from one row per filter, so the variants, their
/// names, argument rules and the cache size cannot fall out of step.
macro_rules! builtin_filters {
    ($($variant:ident => $name:literal, $argument:ident;)+) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(super) enum BuiltinFilter {
            $($variant,)+
        }

        impl BuiltinFilter {
            /// The number of builtin filters, for sizing a cache indexed by
            /// [`BuiltinFilter::index`].
            pub(super) const COUNT: usize = [$(Self::$variant),+].len();

            pub(super) fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub(super) const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            pub(super) const fn argument(self) -> FilterArgument {
                match self {
                    $(Self::$variant => FilterArgument::$argument,)+
                }
            }

            /// A unique index below [`BuiltinFilter::COUNT`].
            pub(super) const fn index(self) -> usize {
                self as usize
            }
        }
    };
}

builtin_filters! {
    Pluralize => "pluralize", Optional;
    Join => "join", Required;
    Ljust => "ljust", Required;
    Rjust => "rjust", Required;
    Slice => "slice", Required;
    Stringformat => "stringformat", Required;
    Truncatechars => "truncatechars", Required;
    Truncatewords => "truncatewords", Required;
    Time => "time", Optional;
    Timesince => "timesince", Optional;
    Timeuntil => "timeuntil", Optional;
    Urlencode => "urlencode", Optional;
    Urlize => "urlize", Forbidden;
    Urlizetrunc => "urlizetrunc", Required;
    JsonScript => "json_script", Optional;
    Floatformat => "floatformat", Optional;
    UnorderedList => "unordered_list", Forbidden;
    MakeList => "make_list", Forbidden;
    Escapeseq => "escapeseq", Forbidden;
    Safeseq => "safeseq", Forbidden;
    Iriencode => "iriencode", Forbidden;
    Random => "random", Forbidden;
    Striptags => "striptags", Forbidden;
    Linebreaks => "linebreaks", Forbidden;
    Linebreaksbr => "linebreaksbr", Forbidden;
    Linenumbers => "linenumbers", Forbidden;
    First => "first", Forbidden;
    Phone2numeric => "phone2numeric", Forbidden;
    TruncatecharsHtml => "truncatechars_html", Required;
    TruncatewordsHtml => "truncatewords_html", Required;
    Dictsort => "dictsort", Required;
    Dictsortreversed => "dictsortreversed", Required;
    GetDigit => "get_digit", Required;
    Filesizeformat => "filesizeformat", Forbidden;
    Pprint => "pprint", Forbidden;
}
