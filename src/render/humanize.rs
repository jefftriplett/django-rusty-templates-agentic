//! Native ports of `django.contrib.humanize`'s filters: `intcomma`,
//! `intword`, `ordinal`, `apnumber`, `naturalday` and `naturaltime`.
//!
//! Most of Django's cost here is translation: `ordinal` looks up ten
//! `pgettext` strings for every value, and `naturaltime` formats lazy
//! `ngettext` strings. [`HumanizeStrings`] keeps each translated string for
//! the active language, still produced by Django's catalogs. Numbers are
//! grouped by the same code `floatformat`'s `g` suffix uses.
//!
//! Only plain values are handled: exact `int`, `float`, `Decimal`, `str`
//! and `datetime` objects. Anything else, a filter argument, and Django
//! versions this port hasn't been checked against call Django's filter.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use num_bigint::BigInt;
use num_traits::Signed;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{
    PyDate, PyDateAccess, PyDateTime, PyDelta, PyDeltaAccess, PyDict, PyFloat, PyInt, PyModule,
    PyString, PyType, PyTzInfoAccess,
};

use crate::filters::python_int;
use crate::filters::{HumanizeFilter, HumanizeKind};
use crate::render::ResolveResult;
use crate::render::dateformat::{Formatter, template_localtime};
use crate::render::filters::{
    DecimalNumber, NumberOptions, ResolveFilter, Rounded, SAFEDATA, Sign, decimal_text,
    floatformat, format_localtime, integer_argument, is_exact_safe_string, number_format,
    python_float_repr,
};
use crate::render::timesince::{TimeSince, TimeStrings, UnitStrings, timesince};
use crate::render::types::{Content, ContentString, Context};
use dtl_lexer::types::TemplateString;

const HUMANIZE: &str = "django.contrib.humanize.templatetags.humanize";

static HUMANIZE_MODULE: PyOnceLock<Py<PyModule>> = PyOnceLock::new();
static DATETIME: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static DECIMAL: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static UTC: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static PGETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static GETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DATE_FILTER: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Django's largest `Decimal` size, digits plus exponent, before
/// `numberformat.format()` switches to scientific notation.
const MAX_DECIMAL_SIZE: usize = 200;

/// `ordinal`'s `pgettext` contexts, by the last digit of the value.
const ORDINAL_CONTEXTS: [&str; 10] = [
    "ordinal 0",
    "ordinal 1",
    "ordinal 2",
    "ordinal 3",
    "ordinal 4",
    "ordinal 5",
    "ordinal 6",
    "ordinal 7",
    "ordinal 8",
    "ordinal 9",
];

/// `apnumber`'s words for 1 to 9.
const AP_NUMBERS: [&str; 9] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

/// A translated `str.format` template such as `"{}st"`.
#[derive(Clone, Debug)]
enum FormatTemplate {
    /// Split around its only `{}`.
    Split(String, String),
    /// Any other template, formatted by Python.
    Python(String),
}

impl FormatTemplate {
    fn new(text: String) -> Self {
        match text.split_once("{}") {
            Some((before, after))
                if !before.contains(['{', '}']) && !after.contains(['{', '}']) =>
            {
                Self::Split(before.to_string(), after.to_string())
            }
            _ => Self::Python(text),
        }
    }

    fn format(&self, py: Python<'_>, value: i64) -> PyResult<String> {
        match self {
            Self::Split(before, after) => Ok(format!("{before}{value}{after}")),
            Self::Python(text) => PyString::new(py, text)
                .call_method1(intern!(py, "format"), (value,))?
                .extract(),
        }
    }
}

/// A translated `"%(value)s million"`-style template.
#[derive(Clone, Debug)]
enum ValueTemplate {
    /// Split around its only `%(value)s`.
    Split(String, String),
    /// Any other template, %-formatted by Python.
    Python(String),
}

impl ValueTemplate {
    fn new(text: String) -> Self {
        match text.split_once("%(value)s") {
            Some((before, after)) if !before.contains('%') && !after.contains('%') => {
                Self::Split(before.to_string(), after.to_string())
            }
            _ => Self::Python(text),
        }
    }

    fn format(&self, py: Python<'_>, value: &str) -> PyResult<String> {
        match self {
            Self::Split(before, after) => Ok(format!("{before}{value}{after}")),
            Self::Python(text) => {
                let arguments = PyDict::new(py);
                arguments.set_item("value", value)?;
                PyString::new(py, text).rem(arguments)?.extract()
            }
        }
    }
}

/// `naturaltime`'s `time_strings` keys handled natively.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum NaturalTime {
    Now,
    PastSecond,
    PastMinute,
    PastHour,
    FutureSecond,
    FutureMinute,
    FutureHour,
}

impl NaturalTime {
    const fn key(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::PastSecond => "past-second",
            Self::PastMinute => "past-minute",
            Self::PastHour => "past-hour",
            Self::FutureSecond => "future-second",
            Self::FutureMinute => "future-minute",
            Self::FutureHour => "future-hour",
        }
    }
}

/// A translated `"%(delta)s ago"` or `"%(delta)s from now"`.
#[derive(Clone, Debug)]
enum SpanTemplate {
    /// The text before and after `%(delta)s`.
    Around(String, String),
    /// The string has other `%`s, which Python formats.
    Python,
}

/// The `humanize` filters' translated strings for one language.
#[derive(Clone, Debug, Default)]
pub struct HumanizeStrings {
    /// `pgettext(context, "{}th")`-style templates, by context.
    ordinals: HashMap<&'static str, FormatTemplate>,
    /// `gettext("one")` to `gettext("nine")`.
    ap_numbers: [Option<String>; 9],
    /// `NaturalTimeFormatter.time_strings[key] % {"count": count}`.
    natural_times: HashMap<(NaturalTime, i32), String>,
    /// `intword_converters[index][1](count)`, by converter and count.
    intwords: HashMap<(usize, i64), ValueTemplate>,
    /// `gettext("today")`, `gettext("tomorrow")` and `gettext("yesterday")`.
    natural_days: [Option<String>; 3],
    /// `naturaltime`'s units for spans of a day or more, past then future.
    natural_units: Option<Box<[TimeStrings; 2]>>,
    /// `time_strings["past-day"]` and `["future-day"]`.
    natural_spans: [Option<SpanTemplate>; 2],
}

impl HumanizeStrings {
    /// `naturaltime` for a span of a day or more. `None` when the
    /// translation needs Python's `%` formatting.
    fn natural_span(
        &mut self,
        py: Python<'_>,
        module: &Bound<'_, PyModule>,
        past: bool,
        partials: Option<[i64; 6]>,
    ) -> PyResult<Option<String>> {
        let index = usize::from(!past);
        let around = match &mut self.natural_spans[index] {
            Some(around) => around,
            vacant @ None => {
                let key = if past { "past-day" } else { "future-day" };
                let template: String = module
                    .getattr(intern!(py, "NaturalTimeFormatter"))?
                    .getattr(intern!(py, "time_strings"))?
                    .get_item(key)?
                    .str()?
                    .extract()?;
                let around = template
                    .split_once("%(delta)s")
                    .filter(|(before, after)| !before.contains('%') && !after.contains('%'))
                    .map_or(SpanTemplate::Python, |(before, after)| {
                        SpanTemplate::Around(before.to_owned(), after.to_owned())
                    });
                vacant.insert(around)
            }
        };
        let SpanTemplate::Around(before, after) = around else {
            return Ok(None);
        };
        let units = self.natural_units.get_or_insert_with(|| {
            Box::new([
                TimeStrings::of(UnitStrings::NaturalPast),
                TimeStrings::of(UnitStrings::NaturalFuture),
            ])
        });
        let delta = units[index].format(py, partials)?;
        Ok(Some(format!("{before}{delta}{after}")))
    }

    fn intword(
        &mut self,
        module: &Bound<'_, PyModule>,
        index: usize,
        count: i64,
    ) -> PyResult<&ValueTemplate> {
        let py = module.py();
        Ok(match self.intwords.entry((index, count)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let text = module
                    .getattr(intern!(py, "intword_converters"))?
                    .get_item(index)?
                    .get_item(1)?
                    .call1((count,))?
                    .extract()?;
                entry.insert(ValueTemplate::new(text))
            }
        })
    }

    fn natural_day(&mut self, py: Python<'_>, index: usize) -> PyResult<&str> {
        let slot = &mut self.natural_days[index];
        if slot.is_none() {
            let message = ["today", "tomorrow", "yesterday"][index];
            let text = GETTEXT
                .import(py, "django.utils.translation", "gettext")?
                .call1((message,))?
                .extract()?;
            *slot = Some(text);
        }
        Ok(slot.as_deref().unwrap_or_default())
    }

    fn ordinal(
        &mut self,
        py: Python<'_>,
        context: &'static str,
        message: &str,
    ) -> PyResult<&FormatTemplate> {
        Ok(match self.ordinals.entry(context) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let text = PGETTEXT
                    .import(py, "django.utils.translation", "pgettext")?
                    .call1((context, message))?
                    .extract()?;
                entry.insert(FormatTemplate::new(text))
            }
        })
    }

    fn ap_number(&mut self, py: Python<'_>, index: usize) -> PyResult<&str> {
        let slot = &mut self.ap_numbers[index];
        if slot.is_none() {
            let text = GETTEXT
                .import(py, "django.utils.translation", "gettext")?
                .call1((AP_NUMBERS[index],))?
                .extract()?;
            *slot = Some(text);
        }
        Ok(slot.as_deref().unwrap_or_default())
    }

    fn natural_time(
        &mut self,
        py: Python<'_>,
        module: &Bound<'_, PyModule>,
        key: NaturalTime,
        count: i32,
    ) -> PyResult<&str> {
        Ok(match self.natural_times.entry((key, count)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let string = module
                    .getattr(intern!(py, "NaturalTimeFormatter"))?
                    .getattr(intern!(py, "time_strings"))?
                    .get_item(key.key())?;
                let text = if key == NaturalTime::Now {
                    string.str()?.extract()?
                } else {
                    let arguments = PyDict::new(py);
                    arguments.set_item("count", count)?;
                    string.rem(arguments)?.extract()?
                };
                entry.insert(text)
            }
        })
    }
}

/// Whether Django marks this input's result safe: the filter is `is_safe`
/// and the input is `SafeData`.
fn is_safe_input(value: &Content<'_, '_>, py: Python<'_>) -> PyResult<bool> {
    Ok(match value {
        Content::String(ContentString::HtmlSafe(_)) => true,
        Content::Py(object) => {
            object.is_instance(SAFEDATA.import(py, "django.utils.safestring", "SafeData")?)?
        }
        _ => false,
    })
}

fn text<'t, 'py>(text: String, safe: bool) -> Content<'t, 'py> {
    Content::String(if safe {
        ContentString::HtmlSafe(Cow::Owned(text))
    } else {
        ContentString::HtmlUnsafe(Cow::Owned(text))
    })
}

/// A plain `str` or `SafeString` value's text.
fn string_value<'a>(value: &'a Content<'_, '_>) -> PyResult<Option<Cow<'a, str>>> {
    Ok(match value {
        Content::String(text) => Some(Cow::Borrowed(text.as_raw().as_ref())),
        Content::Py(object)
            if object.is_exact_instance_of::<PyString>() || is_exact_safe_string(object)? =>
        {
            Some(Cow::Owned(object.cast::<PyString>()?.to_str()?.to_string()))
        }
        _ => None,
    })
}

/// `"{:f}".format(Decimal(...))`, split into sign, integer and fraction.
/// `None` beyond the size Django formats in scientific notation.
fn fixed_point(number: &DecimalNumber) -> Option<Rounded> {
    let leading_zeros = number
        .digits
        .iter()
        .take_while(|&&digit| digit == 0)
        .count()
        .min(number.digits.len().saturating_sub(1));
    let coefficient = &number.digits[leading_zeros..];
    if coefficient.is_empty() {
        return None;
    }
    let exponent = number.exponent.unsigned_abs();
    if usize::try_from(exponent).ok()? + coefficient.len() > MAX_DECIMAL_SIZE {
        return None;
    }
    let to_text = |digits: &[u8]| -> String {
        digits
            .iter()
            .map(|&digit| char::from(b'0' + digit))
            .collect()
    };
    let (integer, fraction) = if number.exponent >= 0 {
        if coefficient == [0] {
            // `Decimal("0E+3")` formats as "0".
            ("0".to_string(), String::new())
        } else {
            let zeros = usize::try_from(number.exponent).ok()?;
            (to_text(coefficient) + &"0".repeat(zeros), String::new())
        }
    } else {
        let places = usize::try_from(exponent).ok()?;
        if coefficient.len() > places {
            let split = coefficient.len() - places;
            (
                to_text(&coefficient[..split]),
                to_text(&coefficient[split..]),
            )
        } else {
            let zeros = "0".repeat(places - coefficient.len());
            ("0".to_string(), zeros + &to_text(coefficient))
        }
    };
    Some(Rounded {
        sign: number.sign,
        integer,
        fraction,
    })
}

fn integer_digits(value: &BigInt) -> Option<Rounded> {
    let integer = value.abs().to_string();
    (integer.len() <= MAX_DECIMAL_SIZE).then(|| Rounded {
        sign: if value.is_negative() {
            Sign::Minus
        } else {
            Sign::Plus
        },
        integer,
        fraction: String::new(),
    })
}

/// The number `intcomma` hands to `number_format()`, as the digits
/// `numberformat.format()` groups. `None` leaves the value to Django.
fn intcomma_number(
    value: &Content<'_, '_>,
    series: (u8, u8),
    py: Python<'_>,
) -> PyResult<Option<Rounded>> {
    // Django 5.2 converts with `int()`; 6.0 and later with `Decimal()`.
    let decimal = series >= (6, 0);
    let float = |number: f64| -> Option<Rounded> {
        if !number.is_finite() {
            return None;
        }
        let repr = python_float_repr(number);
        // `numberformat.format()` treats exponent floats as `Decimal(str(f))`.
        if repr.contains('e') {
            return fixed_point(&DecimalNumber::parse(&repr)?);
        }
        let (sign, repr) = match repr.strip_prefix('-') {
            Some(repr) => (Sign::Minus, repr),
            None => (Sign::Plus, repr.as_str()),
        };
        let (integer, fraction) = repr.split_once('.')?;
        Some(Rounded {
            sign,
            integer: integer.to_string(),
            fraction: fraction.to_string(),
        })
    };
    Ok(match value {
        Content::Int(number) => integer_digits(number),
        Content::Float(number) => float(*number),
        Content::Bool(_) => None,
        Content::Py(object) if object.is_exact_instance_of::<PyInt>() => {
            integer_digits(&object.extract()?)
        }
        Content::Py(object) if object.is_exact_instance_of::<PyFloat>() => float(object.extract()?),
        Content::Py(object)
            if object
                .get_type()
                .is(DECIMAL.import(py, "decimal", "Decimal")?) =>
        {
            let text = object.str()?;
            DecimalNumber::parse(text.to_str()?)
                .as_ref()
                .and_then(fixed_point)
        }
        _ => match string_value(value)? {
            Some(text) if decimal => decimal_text(&text)
                .and_then(DecimalNumber::parse)
                .as_ref()
                .and_then(fixed_point),
            Some(text) => python_int(&text).and_then(|number| integer_digits(&number.into())),
            None => None,
        },
    })
}

fn intcomma<'t, 'py>(
    value: &Content<'t, 'py>,
    series: (u8, u8),
    py: Python<'py>,
    context: &mut Context,
) -> PyResult<Option<Content<'t, 'py>>> {
    let Some(number) = intcomma_number(value, series, py)? else {
        return Ok(None);
    };
    let options = NumberOptions {
        localize: true,
        force_grouping: true,
    };
    let Some(format) = number_format(py, context, options)? else {
        return Ok(None);
    };
    let formatted = format.format(&number);
    Ok(Some(text(formatted, is_safe_input(value, py)?)))
}

fn ordinal<'t, 'py>(
    value: &Content<'t, 'py>,
    series: (u8, u8),
    py: Python<'py>,
    context: &mut Context,
) -> PyResult<Option<Content<'t, 'py>>> {
    let Some(number) = integer_argument(value) else {
        return Ok(None);
    };
    if number < 0 {
        return Ok(Some(text(number.to_string(), is_safe_input(value, py)?)));
    }
    let (pgettext_context, message) = if series >= (6, 1) && number == 1 {
        ("ordinal is 1", "{}st")
    } else if matches!(number % 100, 11..=13) {
        ("ordinal 11, 12, 13", "{}th")
    } else {
        let digit = usize::try_from(number % 10).unwrap_or_default();
        let message = match digit {
            1 => "{}st",
            2 => "{}nd",
            3 => "{}rd",
            _ => "{}th",
        };
        (ORDINAL_CONTEXTS[digit], message)
    };
    let Some(strings) = context
        .caches()
        .humanize_strings
        .get_or_try_init(py, |_| Ok(HumanizeStrings::default()))?
    else {
        return Ok(None);
    };
    // Django builds all ten templates for any other value; each `pgettext`
    // call is pure, so looking up only the one used gives the same text.
    let formatted = strings
        .ordinal(py, pgettext_context, message)?
        .format(py, number)?;
    // `ordinal` always marks its result safe.
    Ok(Some(text(formatted, true)))
}

fn apnumber<'t, 'py>(
    value: &Content<'t, 'py>,
    py: Python<'py>,
    context: &mut Context,
) -> PyResult<Option<Content<'t, 'py>>> {
    let Some(number) = integer_argument(value) else {
        return Ok(None);
    };
    let safe = is_safe_input(value, py)?;
    if !(1..10).contains(&number) {
        // Django returns `int(value)`; `mark_safe` turns it into a string.
        return Ok(Some(if safe {
            text(number.to_string(), true)
        } else {
            // A Python int, so printing it localizes as Django does.
            Content::Py(number.into_pyobject(py)?.into_any())
        }));
    }
    let Some(strings) = context
        .caches()
        .humanize_strings
        .get_or_try_init(py, |_| Ok(HumanizeStrings::default()))?
    else {
        return Ok(None);
    };
    let index = usize::try_from(number - 1).unwrap_or_default();
    Ok(Some(text(strings.ap_number(py, index)?.to_string(), safe)))
}

fn naturaltime<'t, 'py>(
    value: &Content<'t, 'py>,
    py: Python<'py>,
    context: &mut Context,
) -> PyResult<Option<Content<'t, 'py>>> {
    let Content::Py(value) = value else {
        return Ok(None);
    };
    if !value.is_exact_instance_of::<PyDateTime>() {
        return Ok(None);
    }
    let module = HUMANIZE_MODULE
        .get_or_try_init(py, || py.import(HUMANIZE).map(Bound::unbind))?
        .bind(py);
    // Tests and tools such as freezegun replace the module's `datetime`;
    // leave those to Django, which reads it on every call.
    let datetime = DATETIME.import(py, "datetime", "datetime")?;
    if !module.getattr(intern!(py, "datetime"))?.is(datetime) {
        return Ok(None);
    }
    let datetime_value = value.cast::<PyDateTime>()?;
    // `is_aware(value)`: `utcoffset()` isn't `None`.
    let aware = datetime_value.get_tzinfo().is_some()
        && !value.call_method0(intern!(py, "utcoffset"))?.is_none();
    let tz = if aware {
        UTC.import(py, "datetime", "UTC")?.clone()
    } else {
        py.None().into_bound(py)
    };
    let now = datetime.call_method1(intern!(py, "now"), (tz,))?;
    let past = value.lt(&now)?;
    let delta = if past {
        now.sub(value)?
    } else {
        value.sub(&now)?
    };
    let delta = delta.cast_into::<PyDelta>()?;
    if delta.get_days() != 0 {
        // `time_strings["past-day"] % {"delta": timesince(value, now,
        // time_strings=past_substrings)}`, or `timeuntil` for the future.
        let Some(TimeSince::Partials(partials)) = timesince(value, Some(&now), !past)? else {
            return Ok(None);
        };
        let Some(strings) = context
            .caches()
            .humanize_strings
            .get_or_try_init(py, |_| Ok(HumanizeStrings::default()))?
        else {
            return Ok(None);
        };
        return strings
            .natural_span(py, module, past, partials)
            .map(|formatted| formatted.map(|formatted| text(formatted, false)));
    }
    let seconds = delta.get_seconds();
    let (key, count) = match (past, seconds) {
        (_, 0) => (NaturalTime::Now, 0),
        (true, 1..60) => (NaturalTime::PastSecond, seconds),
        (false, 1..60) => (NaturalTime::FutureSecond, seconds),
        (true, _) if seconds / 60 < 60 => (NaturalTime::PastMinute, seconds / 60),
        (false, _) if seconds / 60 < 60 => (NaturalTime::FutureMinute, seconds / 60),
        (true, _) => (NaturalTime::PastHour, seconds / 3600),
        (false, _) => (NaturalTime::FutureHour, seconds / 3600),
    };
    let Some(strings) = context
        .caches()
        .humanize_strings
        .get_or_try_init(py, |_| Ok(HumanizeStrings::default()))?
    else {
        return Ok(None);
    };
    let formatted = strings.natural_time(py, module, key, count)?.to_string();
    // `naturaltime` isn't `is_safe`, so its text is escaped.
    Ok(Some(text(formatted, false)))
}

fn humanize_module(py: Python<'_>) -> PyResult<&Bound<'_, PyModule>> {
    Ok(HUMANIZE_MODULE
        .get_or_try_init(py, || py.import(HUMANIZE).map(Bound::unbind))?
        .bind(py))
}

/// `round_away_from_one(value)`: `int(Decimal(value - 1).quantize(
/// Decimal("0"), rounding=ROUND_UP)) + 1`.
fn round_away_from_one(value: f64) -> i64 {
    let shifted = value - 1.0;
    // `ROUND_UP` rounds away from zero.
    let rounded = if shifted >= 0.0 {
        shifted.ceil()
    } else {
        shifted.floor()
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "callers pass values below 1,000 in magnitude"
    )]
    let rounded = rounded as i64;
    rounded + 1
}

/// Integers up to 2⁵³ convert to `f64` exactly, so `value / 10**exponent`
/// rounds once, as Python's `int / int` does.
const EXACT_FLOAT_LIMIT: i64 = 1 << 53;

fn intword<'t, 'py>(
    value: &Content<'t, 'py>,
    py: Python<'py>,
    context: &mut Context,
) -> PyResult<Option<Content<'t, 'py>>> {
    let Some(number) = integer_argument(value) else {
        return Ok(None);
    };
    let magnitude = number.unsigned_abs();
    if magnitude < 1_000_000 {
        // Django returns `int(value)`, which prints localized.
        return Ok(Some(Content::Py(number.into_pyobject(py)?.into_any())));
    }
    if number.unsigned_abs() >= EXACT_FLOAT_LIMIT.unsigned_abs() {
        return Ok(None);
    }
    // `intword_converters` from a million to a quadrillion; larger values
    // are past the exact float range.
    let Some((index, exponent)) = [6, 9, 12, 15]
        .into_iter()
        .enumerate()
        .find(|&(_, exponent)| magnitude < 10_u64.pow(exponent) * 1000)
    else {
        return Ok(None);
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "values below 2⁵³ convert exactly"
    )]
    let new_value = number as f64 / 10_f64.powi(i32::try_from(exponent).unwrap_or_default());
    let rounded = round_away_from_one(new_value);
    let options = NumberOptions {
        localize: true,
        force_grouping: false,
    };
    let Some(formatted) = floatformat(&Content::Float(new_value), 1, options, py, context)? else {
        return Ok(None);
    };
    let module = humanize_module(py)?;
    let Some(strings) = context
        .caches()
        .humanize_strings
        .get_or_try_init(py, |_| Ok(HumanizeStrings::default()))?
    else {
        return Ok(None);
    };
    let words = strings
        .intword(module, index, rounded.abs())?
        .format(py, &formatted)?;
    // `intword` isn't `is_safe`, so its text is escaped.
    Ok(Some(text(words, false)))
}

/// Days from 0001-01-01 in the proleptic Gregorian calendar.
fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era
}

fn naturalday<'t, 'py>(
    value: &Content<'t, 'py>,
    argument: Option<Content<'t, 'py>>,
    py: Python<'py>,
    context: &mut Context,
) -> PyResult<Option<Content<'t, 'py>>> {
    let Content::Py(value) = value else {
        return Ok(None);
    };
    let is_datetime = value.is_exact_instance_of::<PyDateTime>();
    if !is_datetime && !value.is_exact_instance_of::<PyDate>() {
        return Ok(None);
    }
    let module = humanize_module(py)?;
    // Tests and tools such as freezegun replace the module's `datetime`;
    // leave those to Django, which reads it on every call.
    let datetime = DATETIME.import(py, "datetime", "datetime")?;
    if !module.getattr(intern!(py, "datetime"))?.is(datetime) {
        return Ok(None);
    }
    // `expects_localtime=True`.
    let value = if is_datetime {
        template_localtime(
            value.clone(),
            context.use_tz,
            &mut context.date_cache.borrow_mut(),
        )?
    } else {
        value.clone()
    };
    let tzinfo = if is_datetime {
        value
            .cast::<PyDateTime>()?
            .get_tzinfo()
            .map_or_else(|| py.None().into_bound(py), Bound::into_any)
    } else {
        py.None().into_bound(py)
    };
    let date = value.cast::<PyDate>()?;
    let (year, month, day) = (date.get_year(), date.get_month(), date.get_day());
    let today = datetime
        .call_method1(intern!(py, "now"), (tzinfo,))?
        .call_method0(intern!(py, "date"))?;
    let today = today.cast::<PyDate>()?;
    let delta = days_from_civil(year, month, day)
        - days_from_civil(today.get_year(), today.get_month(), today.get_day());
    let index = match delta {
        0 => Some(0),
        1 => Some(1),
        -1 => Some(2),
        _ => None,
    };
    if let Some(index) = index {
        let Some(strings) = context
            .caches()
            .humanize_strings
            .get_or_try_init(py, |_| Ok(HumanizeStrings::default()))?
        else {
            return Ok(None);
        };
        let day = strings.natural_day(py, index)?.to_string();
        return Ok(Some(text(day, false)));
    }
    // `defaultfilters.date(date(value.year, value.month, value.day), arg)`.
    let plain_date = PyDate::new(py, year, month, day)?.into_any();
    let date_filter = DATE_FILTER.import(py, "django.template.defaultfilters", "date")?;
    Ok(Some(format_localtime(
        plain_date,
        argument,
        Formatter::Date,
        date_filter,
        context,
    )?))
}

/// The Django versions whose `humanize` code this port follows.
const fn supported(series: (u8, u8)) -> bool {
    matches!(series, (5, 2) | (6, 0..=2))
}

impl ResolveFilter for HumanizeFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let series = context.django_series(py)?;
        if self.kind == HumanizeKind::Naturalday {
            // The only one with an argument: resolve it once for both paths.
            let argument = self.django.resolve_argument(py, template, context)?;
            if let Some(value) = &variable
                && supported(series)
                && let Some(content) = naturalday(
                    value,
                    argument.value().map(|format| Content::Py(format.to_py(py))),
                    py,
                    context,
                )?
            {
                return Ok(Some(content));
            }
            return self.django.call(variable, argument, py, context);
        }
        if let Some(value) = &variable
            && self.django.argument.is_none()
            && supported(series)
        {
            let native = match self.kind {
                HumanizeKind::Intcomma => intcomma(value, series, py, context)?,
                HumanizeKind::Ordinal => ordinal(value, series, py, context)?,
                HumanizeKind::Apnumber => apnumber(value, py, context)?,
                HumanizeKind::Intword => intword(value, py, context)?,
                HumanizeKind::Naturaltime => naturaltime(value, py, context)?,
                HumanizeKind::Naturalday => None,
            };
            if let Some(content) = native {
                return Ok(Some(content));
            }
        }
        self.django.resolve(variable, py, template, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(text: &str) -> Option<String> {
        let rounded = fixed_point(&DecimalNumber::parse(text)?)?;
        let sign = if rounded.sign == Sign::Minus { "-" } else { "" };
        let fraction = if rounded.fraction.is_empty() {
            String::new()
        } else {
            format!(".{}", rounded.fraction)
        };
        Some(format!("{sign}{}{fraction}", rounded.integer))
    }

    #[test]
    fn test_fixed_point_matches_python_format() {
        // `"{:f}".format(Decimal(text))`.
        for (text, expected) in [
            ("0E+3", "0"),
            ("1E+3", "1000"),
            ("0E-5", "0.00000"),
            ("-0", "-0"),
            ("0012", "12"),
            ("0.05", "0.05"),
            ("1.2E+3", "1200"),
            ("-0.000", "-0.000"),
            ("12.3400", "12.3400"),
            ("1e-7", "0.0000001"),
            ("1.5e+20", "150000000000000000000"),
        ] {
            assert_eq!(fixed(text).as_deref(), Some(expected), "{text}");
        }
    }

    #[test]
    fn test_fixed_point_leaves_huge_numbers_to_django() {
        assert_eq!(fixed("1e200"), None);
        assert_eq!(fixed("1e199").map(|text| text.len()), Some(200));
    }

    #[test]
    fn test_format_template() {
        Python::initialize();
        Python::attach(|py| {
            for (template, expected) in [
                ("{}st", "21st"),
                ("{}.", "21."),
                ("{0}e", "21e"),
                ("{}{{x}}", "21{x}"),
            ] {
                let formatted = FormatTemplate::new(template.to_string())
                    .format(py, 21)
                    .expect("format should work");
                assert_eq!(formatted, expected, "{template}");
            }
        });
    }
}
