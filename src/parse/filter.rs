//! Parsing of variable filters into `Filter` nodes.
use crate::filters::AddFilter;
use crate::filters::AddSlashesFilter;
use crate::filters::CapfirstFilter;
use crate::filters::CenterFilter;
use crate::filters::CutFilter;
use crate::filters::DateFilter;
use crate::filters::DefaultFilter;
use crate::filters::DefaultIfNoneFilter;
use crate::filters::DictsortFilter;
use crate::filters::DictsortreversedFilter;
use crate::filters::DivisibleByFilter;
use crate::filters::DjangoFilter;
use crate::filters::EscapeFilter;
use crate::filters::EscapejsFilter;
use crate::filters::EscapeseqFilter;
use crate::filters::ExternalFilter;
use crate::filters::FilesizeformatFilter;
use crate::filters::FilterType;
use crate::filters::FirstFilter;
use crate::filters::FloatFormatFilter;
use crate::filters::ForceEscapeFilter;
use crate::filters::GetDigitFilter;
use crate::filters::HumanizeFilter;
use crate::filters::HumanizeKind;
use crate::filters::IriencodeFilter;
use crate::filters::JoinFilter;
use crate::filters::JsonScriptFilter;
use crate::filters::Justify;
use crate::filters::JustifyFilter;
use crate::filters::L10nFilter;
use crate::filters::LanguageField;
use crate::filters::LanguageInfoFilter;
use crate::filters::LastFilter;
use crate::filters::LengthFilter;
use crate::filters::LinebreaksFilter;
use crate::filters::LinebreaksbrFilter;
use crate::filters::LinenumbersFilter;
use crate::filters::LowerFilter;
use crate::filters::MakeListFilter;
use crate::filters::Phone2numericFilter;
use crate::filters::PluralizeFilter;
use crate::filters::PprintFilter;
use crate::filters::RandomFilter;
use crate::filters::SafeFilter;
use crate::filters::SafeseqFilter;
use crate::filters::SliceArgument;
use crate::filters::SliceFilter;
use crate::filters::SlugifyFilter;
use crate::filters::StringformatFilter;
use crate::filters::StriptagsFilter;
use crate::filters::TimesinceFilter;
use crate::filters::TitleFilter;
use crate::filters::TruncateCharsFilter;
use crate::filters::TruncateCharsHtmlFilter;
use crate::filters::TruncateWordsFilter;
use crate::filters::TruncateWordsHtmlFilter;
use crate::filters::TzConversion;
use crate::filters::TzFilter;
use crate::filters::UnorderedListFilter;
use crate::filters::UpperFilter;
use crate::filters::UrlencodeFilter;
use crate::filters::UrlizeFilter;
use crate::filters::UrlizetruncFilter;
use crate::filters::WordcountFilter;
use crate::filters::WordwrapFilter;
use crate::filters::YesnoFilter;
use crate::gc::Gc;
use crate::parse::ParseError;
use crate::parse::Parser;
use crate::parse::TagElement;
use crate::parse::builtin_filter::BuiltinFilter;
use crate::parse::builtin_filter::FilterArgument;
use crate::types::Argument;
use crate::types::ArgumentType;
use dtl_lexer::types::At;
use num_traits::ToPrimitive;
use pyo3::intern;
use pyo3::prelude::*;

fn unexpected_argument(filter: &'static str, right: Argument) -> ParseError {
    ParseError::UnexpectedArgument {
        filter,
        at: right.at.into(),
    }
}

/// Whether `filter` is Django's own implementation of `name`, so a native
/// port may stand in for it, rather than a replacement registered in its place.
fn is_django_filter(py: Python<'_>, filter: &Py<PyAny>, name: &str) -> bool {
    py.import(intern!(py, "django.template.defaultfilters"))
        .and_then(|module| module.getattr(name))
        .is_ok_and(|django| django.is(filter))
}

/// A filter from a loaded library. Django's own `{% load tz %}` filters get a
/// native port; anything else, including replacements registered under the
/// same names, calls the Python function.
/// Finds which of `module`'s functions, if any, `filter` is.
fn stock_filter<T: Copy>(
    py: Python<'_>,
    filter: &Bound<'_, PyAny>,
    module: &str,
    names: &[(&str, T)],
) -> Option<T> {
    let module = py.import(module).ok()?;
    names
        .iter()
        .find(|(name, _)| module.getattr(*name).is_ok_and(|django| django.is(filter)))
        .map(|(_, kind)| *kind)
}

const HUMANIZE: &str = "django.contrib.humanize.templatetags.humanize";

/// A library filter, natively handled when it is one of Django's own
/// `{% load tz %}`, `{% load l10n %}`, `{% load i18n %}` or
/// `{% load humanize %}` filters.
fn external_filter(py: Python<'_>, filter: Py<PyAny>, argument: Option<Argument>) -> FilterType {
    let bound = filter.bind(py);
    let tz = stock_filter(
        py,
        bound,
        "django.templatetags.tz",
        &[
            ("localtime", TzConversion::Localtime),
            ("utc", TzConversion::Utc),
            ("do_timezone", TzConversion::Timezone),
        ],
    );
    let l10n = || {
        stock_filter(
            py,
            bound,
            "django.templatetags.l10n",
            &[("localize", true), ("unlocalize", false)],
        )
    };
    let i18n = || {
        stock_filter(
            py,
            bound,
            "django.templatetags.i18n",
            &[
                ("language_name", LanguageField::Name),
                ("language_name_local", LanguageField::NameLocal),
                ("language_name_translated", LanguageField::NameTranslated),
                ("language_bidi", LanguageField::Bidi),
            ],
        )
    };
    // Only check humanize when the filter comes from it, so templates that
    // don't use it never import the module.
    let humanize = || {
        let from_humanize = bound
            .getattr(intern!(py, "__module__"))
            .is_ok_and(|module| module.eq(HUMANIZE).unwrap_or(false));
        if !from_humanize {
            return None;
        }
        stock_filter(
            py,
            bound,
            HUMANIZE,
            &[
                ("apnumber", HumanizeKind::Apnumber),
                ("intcomma", HumanizeKind::Intcomma),
                ("intword", HumanizeKind::Intword),
                ("naturalday", HumanizeKind::Naturalday),
                ("naturaltime", HumanizeKind::Naturaltime),
                ("ordinal", HumanizeKind::Ordinal),
            ],
        )
    };
    let django = || DjangoFilter {
        filter: Gc::new(filter.clone_ref(py)),
        argument: argument.clone(),
    };
    if let Some(conversion) = tz {
        FilterType::Tz(TzFilter {
            conversion,
            django: django(),
        })
    } else if let Some(localize) = l10n() {
        FilterType::L10n(L10nFilter {
            localize,
            django: django(),
        })
    } else if let Some(kind) = humanize() {
        FilterType::Humanize(HumanizeFilter {
            kind,
            django: django(),
        })
    } else if let Some(field) = i18n() {
        FilterType::LanguageInfo(LanguageInfoFilter {
            field,
            django: django(),
        })
    } else {
        FilterType::External(ExternalFilter::new(filter, argument))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub at: At,
    pub all_at: At,
    pub left: TagElement,
    pub filter: FilterType,
}

/// Django's `FilterExpression.args_check`: a filter function takes the
/// value, plus the argument when it has a parameter for one. Django counts
/// the positional parameters of the function a decorator wraps.
fn check_filter_arguments(
    filter: &Bound<'_, PyAny>,
    name: &str,
    has_argument: bool,
    at: At,
) -> Result<(), ParseError> {
    let py = filter.py();
    let mut function = filter.clone();
    while let Ok(Some(wrapped)) = function.getattr_opt(intern!(py, "__wrapped__")) {
        function = wrapped;
    }
    let bound_self = function
        .getattr_opt(intern!(py, "__self__"))
        .ok()
        .flatten()
        .is_some_and(|owner| !owner.is_none());
    // Not a plain function (a class, a builtin): leave it to the call.
    let Some(parameters) = function
        .getattr_opt(intern!(py, "__code__"))
        .ok()
        .flatten()
        .and_then(|code| code.getattr(intern!(py, "co_argcount")).ok())
        .and_then(|count| count.extract::<usize>().ok())
    else {
        return Ok(());
    };
    let parameters = parameters.saturating_sub(usize::from(bound_self));
    let defaults = function
        .getattr_opt(intern!(py, "__defaults__"))
        .ok()
        .flatten()
        .and_then(|defaults| defaults.len().ok())
        .unwrap_or(0);
    let required = parameters.saturating_sub(defaults);
    let provided = 1 + usize::from(has_argument);
    if provided < required || provided > parameters {
        return Err(ParseError::FilterArguments {
            name: name.to_string(),
            required,
            provided,
            at: at.into(),
        });
    }
    Ok(())
}

impl Filter {
    #[expect(clippy::too_many_lines, reason = "one match arm per builtin filter")]
    pub fn new(
        parser: &Parser,
        at: At,
        all_at: At,
        left: TagElement,
        right: Option<Argument>,
    ) -> Result<Self, ParseError> {
        if let Some(filter) = parser.external_filters.get(parser.template.content(at)) {
            check_filter_arguments(filter, parser.template.content(at), right.is_some(), at)?;
            return Ok(Self {
                at,
                all_at,
                left,
                filter: external_filter(parser.py, filter.clone().unbind(), right),
            });
        }
        if let Some(builtin) = BuiltinFilter::from_name(parser.template.content(at)) {
            let argument = match (builtin.argument(), right) {
                (FilterArgument::Required, None) => {
                    return Err(ParseError::MissingArgument { at: at.into() });
                }
                (FilterArgument::Forbidden, Some(right)) => {
                    return Err(unexpected_argument(builtin.name(), right));
                }
                (FilterArgument::Optional | FilterArgument::Required, right) => right,
                (FilterArgument::Forbidden, None) => None,
            };
            let django = DjangoFilter {
                filter: parser.builtin_filter(builtin, at)?,
                argument,
            };
            let filter = match builtin {
                BuiltinFilter::Truncatechars
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::TruncateChars(TruncateCharsFilter { django })
                }
                BuiltinFilter::TruncatecharsHtml
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::TruncateCharsHtml(TruncateCharsHtmlFilter { django })
                }
                BuiltinFilter::Truncatewords
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::TruncateWords(TruncateWordsFilter { django })
                }
                BuiltinFilter::Floatformat
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::FloatFormat(FloatFormatFilter { django })
                }
                BuiltinFilter::Slice
                    if is_django_filter(parser.py, &django.filter, "slice_filter") =>
                {
                    let literal = django.argument.as_ref().and_then(|argument| {
                        match &argument.argument_type {
                            ArgumentType::Text(text) => {
                                SliceArgument::parse(parser.template.content(text.at))
                            }
                            ArgumentType::Int(n) => n.to_isize().map(SliceArgument::stop),
                            // `str()` of a float never converts with `int()`.
                            ArgumentType::Float(_) => Some(SliceArgument::Invalid),
                            ArgumentType::Variable(_)
                            | ArgumentType::ForVariable(_)
                            | ArgumentType::TranslatedText(_) => None,
                        }
                    });
                    FilterType::Slice(SliceFilter { django, literal })
                }
                BuiltinFilter::Ljust
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Justify(JustifyFilter {
                        django,
                        side: Justify::Left,
                    })
                }
                BuiltinFilter::Rjust
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Justify(JustifyFilter {
                        django,
                        side: Justify::Right,
                    })
                }
                BuiltinFilter::First
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::First(FirstFilter)
                }
                BuiltinFilter::Join
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Join(JoinFilter { django })
                }
                // `defaultfilters.linebreaks` is the `django.utils.html` helper;
                // the registered filter is `linebreaks_filter`.
                BuiltinFilter::Linebreaks
                    if is_django_filter(parser.py, &django.filter, "linebreaks_filter") =>
                {
                    FilterType::Linebreaks(LinebreaksFilter)
                }
                BuiltinFilter::Linebreaksbr
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Linebreaksbr(LinebreaksbrFilter)
                }
                BuiltinFilter::Linenumbers
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Linenumbers(LinenumbersFilter)
                }
                BuiltinFilter::Timesince
                    if is_django_filter(parser.py, &django.filter, "timesince_filter") =>
                {
                    FilterType::Timesince(TimesinceFilter {
                        django,
                        until: false,
                    })
                }
                BuiltinFilter::Timeuntil
                    if is_django_filter(parser.py, &django.filter, "timeuntil_filter") =>
                {
                    FilterType::Timesince(TimesinceFilter {
                        django,
                        until: true,
                    })
                }
                BuiltinFilter::Pluralize
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Pluralize(PluralizeFilter { django })
                }
                BuiltinFilter::GetDigit
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::GetDigit(GetDigitFilter { django })
                }
                BuiltinFilter::Phone2numeric
                    if is_django_filter(parser.py, &django.filter, "phone2numeric_filter") =>
                {
                    FilterType::Phone2numeric(Phone2numericFilter { django })
                }
                BuiltinFilter::Urlencode
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Urlencode(UrlencodeFilter { django })
                }
                BuiltinFilter::Iriencode
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Iriencode(IriencodeFilter { django })
                }
                BuiltinFilter::MakeList
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::MakeList(MakeListFilter { django })
                }
                BuiltinFilter::Escapeseq
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Escapeseq(EscapeseqFilter { django })
                }
                BuiltinFilter::Safeseq
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Safeseq(SafeseqFilter { django })
                }
                BuiltinFilter::Random
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Random(RandomFilter { django })
                }
                BuiltinFilter::Pprint
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Pprint(PprintFilter { django })
                }
                BuiltinFilter::Filesizeformat
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Filesizeformat(FilesizeformatFilter { django })
                }
                BuiltinFilter::UnorderedList
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::UnorderedList(UnorderedListFilter { django })
                }
                BuiltinFilter::Striptags
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Striptags(StriptagsFilter { django })
                }
                BuiltinFilter::Stringformat
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Stringformat(StringformatFilter { django })
                }
                BuiltinFilter::Dictsort
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Dictsort(DictsortFilter { django })
                }
                BuiltinFilter::Dictsortreversed
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Dictsortreversed(DictsortreversedFilter { django })
                }
                BuiltinFilter::JsonScript
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::JsonScript(JsonScriptFilter { django })
                }
                BuiltinFilter::TruncatewordsHtml
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::TruncateWordsHtml(TruncateWordsHtmlFilter { django })
                }
                BuiltinFilter::Urlize
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Urlize(UrlizeFilter { django })
                }
                BuiltinFilter::Urlizetrunc
                    if is_django_filter(parser.py, &django.filter, builtin.name()) =>
                {
                    FilterType::Urlizetrunc(UrlizetruncFilter { django })
                }
                _ => FilterType::Django(django),
            };
            return Ok(Self {
                at,
                all_at,
                left,
                filter,
            });
        }
        let filter = match parser.template.content(at) {
            "add" => match right {
                Some(right) => FilterType::Add(AddFilter::new(right)),
                None => return Err(ParseError::MissingArgument { at: at.into() }),
            },
            "addslashes" => match right {
                Some(right) => return Err(unexpected_argument("addslashes", right)),
                None => FilterType::AddSlashes(AddSlashesFilter),
            },
            "capfirst" => match right {
                Some(right) => return Err(unexpected_argument("capfirst", right)),
                None => FilterType::Capfirst(CapfirstFilter),
            },
            "center" => match right {
                Some(right) => FilterType::Center(CenterFilter::new(right)),
                None => return Err(ParseError::MissingArgument { at: at.into() }),
            },
            "cut" => match right {
                Some(right) => FilterType::Cut(CutFilter::new(right)),
                None => return Err(ParseError::MissingArgument { at: at.into() }),
            },
            "default" => match right {
                Some(right) => FilterType::Default(DefaultFilter::new(right, at)),
                None => return Err(ParseError::MissingArgument { at: at.into() }),
            },
            "default_if_none" => match right {
                Some(right) => FilterType::DefaultIfNone(DefaultIfNoneFilter::new(right)),
                None => return Err(ParseError::MissingArgument { at: at.into() }),
            },
            "divisibleby" => match right {
                Some(right) => FilterType::DivisibleBy(DivisibleByFilter::new(at, right)),
                None => return Err(ParseError::MissingArgument { at: at.into() }),
            },
            "date" => FilterType::Date(DateFilter::new(right, at)),
            "escape" => match right {
                Some(right) => return Err(unexpected_argument("escape", right)),
                None => FilterType::Escape(EscapeFilter),
            },
            "escapejs" => match right {
                Some(right) => return Err(unexpected_argument("escapejs", right)),
                None => FilterType::Escapejs(EscapejsFilter),
            },
            "force_escape" => match right {
                Some(right) => return Err(unexpected_argument("force_escape", right)),
                None => FilterType::ForceEscape(ForceEscapeFilter),
            },
            "last" => match right {
                Some(right) => return Err(unexpected_argument("last", right)),
                None => FilterType::Last(LastFilter::new(at)),
            },
            "lower" => match right {
                Some(right) => return Err(unexpected_argument("lower", right)),
                None => FilterType::Lower(LowerFilter),
            },
            "length" => match right {
                Some(right) => return Err(unexpected_argument("length", right)),
                None => FilterType::Length(LengthFilter),
            },
            "safe" => match right {
                Some(right) => return Err(unexpected_argument("safe", right)),
                None => FilterType::Safe(SafeFilter),
            },
            "slugify" => match right {
                Some(right) => return Err(unexpected_argument("slugify", right)),
                None => FilterType::Slugify(SlugifyFilter),
            },
            "title" => match right {
                Some(right) => return Err(unexpected_argument("title", right)),
                None => FilterType::Title(TitleFilter),
            },
            "upper" => match right {
                Some(right) => return Err(unexpected_argument("upper", right)),
                None => FilterType::Upper(UpperFilter),
            },
            "wordcount" => match right {
                Some(right) => return Err(unexpected_argument("wordcount", right)),
                None => FilterType::Wordcount(WordcountFilter),
            },
            "wordwrap" => match right {
                Some(right) => FilterType::Wordwrap(WordwrapFilter::new(right)),
                None => return Err(ParseError::MissingArgument { at: at.into() }),
            },
            "yesno" => FilterType::Yesno(YesnoFilter::new(at, right)),
            name => {
                let external = match parser.external_filters.get(name) {
                    Some(external) => {
                        check_filter_arguments(external, name, right.is_some(), at)?;
                        external.clone().unbind()
                    }
                    None => {
                        return Err(ParseError::InvalidFilter {
                            at: at.into(),
                            filter: name.to_string(),
                        });
                    }
                };
                external_filter(parser.py, external, right)
            }
        };
        Ok(Self {
            at,
            all_at,
            left,
            filter,
        })
    }
}
