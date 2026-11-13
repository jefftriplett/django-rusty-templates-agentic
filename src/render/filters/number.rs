//! Number filters: `add`, `divisibleby`, `get_digit`, `floatformat` and `filesizeformat`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use num_bigint::BigInt;
use num_traits::Zero;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyDict, PyFloat, PyInt, PyList, PyString, PyTuple, PyType};

use super::escape::is_exact_safe_string;
use super::python::ResolvedArgument;
use super::{ResolveFilter, SAFEDATA, integer_argument};
use crate::error::AnnotatePyErr;
use crate::filters::{
    AddFilter, DivisibleByFilter, FilesizeformatFilter, FloatFormatFilter, GetDigitFilter,
    python_int,
};
use crate::render::l10n::Grouping;
use crate::render::types::{Content, ContentString, Context};
use crate::render::{Resolve, ResolveFailures, ResolveResult};
use dtl_lexer::types::TemplateString;

static GET_FORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DECIMAL: PyOnceLock<Py<PyType>> = PyOnceLock::new();

/// Python's `int(value)`, with its own error for a value it can't convert.
fn python_int_of(value: &Content<'_, '_>, py: Python<'_>) -> PyResult<BigInt> {
    match value {
        Content::Int(number) => Ok(number.clone()),
        Content::Bool(flag) => Ok(BigInt::from(u8::from(*flag))),
        value => PyType::new::<PyInt>(py)
            .call1((value.to_py(py),))?
            .extract(),
    }
}

impl ResolveFilter for AddFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Resolved first: a missing argument raises even for a missing value.
        let right = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?
            .expect("missing argument in context should already have raised");
        let Some(variable) = variable else {
            return Ok(None);
        };
        // `int()` of an infinity raises `OverflowError`, which Django's
        // `add` doesn't catch.
        if variable.is_infinite() || (variable.to_bigint().is_some() && right.is_infinite()) {
            return Err(pyo3::exceptions::PyOverflowError::new_err(
                "cannot convert float infinity to integer",
            )
            .annotate(py, self.argument.at, "here", template)
            .into());
        }
        // Django gives up on `int()` after the first failure.
        let integers = variable
            .to_bigint()
            .and_then(|left| Some((left, right.to_bigint()?)));
        Ok(match integers {
            Some((variable, right)) => Some(Content::Int(variable + right)),
            None => {
                let variable = variable.to_py(py);
                let right = right.to_py(py);
                variable.add(right).ok().map(Content::Py)
            }
        })
    }
}

impl ResolveFilter for DivisibleByFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Django resolves a filter's arguments before calling it, so a
        // missing argument raises even for a missing value.
        let argument = self
            .argument
            .resolve(py, template, context, ResolveFailures::Raise)?;
        let Some(variable) = variable else {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "invalid literal for int() with base 10: 'None'",
            )
            .annotate(py, self.at, "here", template)
            .into());
        };

        if let Content::Py(ref obj) = variable
            && obj.is_none()
        {
            return Err(pyo3::exceptions::PyTypeError::new_err(
                    "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'",
                )
                .annotate(py, self.at, "here", template)
                .into());
        }

        let right = argument.expect("missing argument in context should already have raised");

        // Django's `int(value) % int(arg) == 0`, in that order, so the
        // value's error comes before the argument's and before a zero.
        let left_val = python_int_of(&variable, py)
            .map_err(|error| error.annotate(py, self.at, "here", template))?;
        let right_val = python_int_of(&right, py)
            .map_err(|error| error.annotate(py, self.argument.at, "here", template))?;
        if right_val.is_zero() {
            return Err(
                pyo3::exceptions::PyZeroDivisionError::new_err("integer modulo by zero")
                    .annotate(py, self.argument.at, "here", template)
                    .into(),
            );
        }

        Ok(Some(Content::Bool((left_val % right_val).is_zero())))
    }
}

/// The number formatting `django.utils.formats.number_format()` reads for
/// the active language.
#[derive(Clone, Debug)]
pub struct FloatNumberFormat {
    decimal_separator: String,
    thousand_separator: String,
    grouping: Grouping,
}

impl FloatNumberFormat {
    /// Returns `None` for settings Django would reject or treat specially,
    /// leaving those to Django.
    fn new(
        py: Python<'_>,
        language: &Bound<'_, PyString>,
        options: NumberOptions,
    ) -> PyResult<Option<Self>> {
        let get_format = GET_FORMAT.import(py, "django.utils.formats", "get_format")?;
        // Unlocalized formats come from settings, as `get_format(name, None,
        // use_l10n=False)` returns them.
        let format = |name| {
            if options.localize {
                get_format.call1((name, language, true))
            } else {
                get_format.call1((name, py.None(), false))
            }
        };
        let (Ok(decimal_separator), Ok(thousand_separator)) = (
            format("DECIMAL_SEPARATOR")?.extract(),
            format("THOUSAND_SEPARATOR")?.extract(),
        ) else {
            return Ok(None);
        };
        let grouping = format("NUMBER_GROUPING")?;
        // `numberformat.format()`: localized output follows
        // `USE_THOUSAND_SEPARATOR`, and `force_grouping` always groups.
        let use_grouping = (options.force_grouping
            || (options.localize
                && SETTINGS
                    .import(py, "django.conf", "settings")?
                    .getattr("USE_THOUSAND_SEPARATOR")?
                    .is_truthy()?))
            && grouping.ne(0)?;
        let grouping = if !use_grouping {
            Grouping::Off
        } else if grouping.is_exact_instance_of::<PyInt>() {
            match grouping.extract() {
                Ok(size) => Grouping::Sizes(vec![size]),
                Err(_) => return Ok(None),
            }
        } else if grouping.is_instance_of::<PyList>() || grouping.is_instance_of::<PyTuple>() {
            // An empty sequence makes Django raise IndexError.
            match grouping.extract::<Vec<i64>>() {
                Ok(sizes) if !sizes.is_empty() => Grouping::Sizes(sizes),
                _ => return Ok(None),
            }
        } else {
            return Ok(None);
        };
        Ok(Some(Self {
            decimal_separator,
            thousand_separator,
            grouping,
        }))
    }

    /// `django.utils.numberformat.format()` for a number already rounded to
    /// its decimal places.
    pub(crate) fn format(&self, number: &Rounded) -> String {
        let Rounded {
            sign,
            integer,
            fraction,
        } = number;
        let mut output = String::with_capacity(integer.len() * 2 + fraction.len() + 2);
        output.push_str(sign.prefix());
        self.grouping
            .push_digits(integer, &self.thousand_separator, &mut output);
        if !fraction.is_empty() {
            output.push_str(&self.decimal_separator);
            output.push_str(fraction);
        }
        output
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    Plus,
    Minus,
}

impl Sign {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Plus => "",
            Self::Minus => "-",
        }
    }
}

/// `number_format()`'s `use_l10n` and `force_grouping`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NumberOptions {
    pub(crate) localize: bool,
    pub(crate) force_grouping: bool,
}

impl Default for NumberOptions {
    fn default() -> Self {
        Self {
            localize: true,
            force_grouping: false,
        }
    }
}

/// The number formats of one language, built for each [`NumberOptions`] on
/// first use. `None` marks settings left to Django.
#[derive(Clone, Debug)]
pub struct FloatNumberFormats {
    language: String,
    formats: HashMap<NumberOptions, Option<FloatNumberFormat>>,
}

impl FloatNumberFormats {
    fn new(language: &Bound<'_, PyString>) -> PyResult<Self> {
        Ok(Self {
            language: language.to_str()?.to_string(),
            formats: HashMap::new(),
        })
    }

    fn get(
        &mut self,
        py: Python<'_>,
        options: NumberOptions,
    ) -> PyResult<Option<&FloatNumberFormat>> {
        let format = match self.formats.entry(options) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(FloatNumberFormat::new(
                py,
                &PyString::new(py, &self.language),
                options,
            )?),
        };
        Ok(format.as_ref())
    }
}

/// The number formatting for the active language and `options`.
pub fn number_format<'c>(
    py: Python<'_>,
    context: &'c mut Context,
    options: NumberOptions,
) -> PyResult<Option<&'c FloatNumberFormat>> {
    match context
        .caches()
        .float_number_format
        .get_or_try_init(py, FloatNumberFormats::new)?
    {
        Some(formats) => formats.get(py, options),
        None => Ok(None),
    }
}

/// A number rounded to its decimal places, ready to localize.
pub struct Rounded {
    pub(crate) sign: Sign,
    pub(crate) integer: String,
    pub(crate) fraction: String,
}

/// A finite decimal number, `digits × 10^exponent`, as `Decimal(str(value))`
/// sees an `int`, `float` or `Decimal`.
pub struct DecimalNumber {
    pub(crate) sign: Sign,
    pub(crate) digits: Vec<u8>,
    pub(crate) exponent: i64,
}

impl DecimalNumber {
    /// Parses Python's `str()` of a number. Infinities and NaNs return `None`.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (sign, text) = match text.strip_prefix('-') {
            Some(text) => (Sign::Minus, text),
            None => (Sign::Plus, text),
        };
        let (mantissa, exponent) = match text.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
            None => (text, 0),
        };
        let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let digits: Vec<u8> = integer
            .bytes()
            .chain(fraction.bytes())
            .map(|byte| byte.is_ascii_digit().then(|| byte - b'0'))
            .collect::<Option<_>>()?;
        if digits.is_empty() {
            return None;
        }
        Some(Self {
            sign,
            digits,
            exponent: exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?,
        })
    }

    /// Rounds half up to `places` decimal places, like `Decimal.quantize()`
    /// with `ROUND_HALF_UP`, which rounds the magnitude half away from zero.
    fn round(&self, places: usize) -> Option<Rounded> {
        let shift = self.exponent.checked_add(i64::try_from(places).ok()?)?;
        let mut digits = self.digits.clone();
        if let Ok(zeros) = usize::try_from(shift) {
            digits.resize(digits.len() + zeros, 0);
        } else {
            let dropped = usize::try_from(shift.unsigned_abs()).ok()?;
            let kept = digits.len().saturating_sub(dropped);
            let round_up = dropped <= digits.len() && digits[kept] >= 5;
            digits.truncate(kept);
            if round_up {
                match digits.iter().rposition(|&digit| digit != 9) {
                    Some(index) => {
                        digits[index] += 1;
                        digits[index + 1..].fill(0);
                    }
                    None => {
                        digits.fill(0);
                        digits.insert(0, 1);
                    }
                }
            }
        }
        // Like Django, a number that rounds to zero loses its sign: -0.01 is "0.0".
        let sign = if digits.iter().all(|&digit| digit == 0) {
            Sign::Plus
        } else {
            self.sign
        };
        let leading_zeros = digits.iter().take_while(|&&digit| digit == 0).count();
        let digits = &digits[leading_zeros..];
        let split = digits.len().saturating_sub(places);
        let to_text = |digits: &[u8]| -> String {
            digits
                .iter()
                .map(|&digit| char::from(b'0' + digit))
                .collect()
        };
        let integer = match split {
            0 => "0".to_string(),
            _ => to_text(&digits[..split]),
        };
        let fraction = format!("{:0>places$}", to_text(&digits[split..]));
        Some(Rounded {
            sign,
            integer,
            fraction,
        })
    }
}

/// Django's limit on digits plus exponent, beyond which `floatformat`
/// returns the input unchanged. Larger precisions are also left to Django.
const FLOATFORMAT_MAX_DIGITS: usize = 200;

/// `floatformat` for exact `int`, `float` and `Decimal` values and an
/// integer precision. `None` defers to Django.
/// `text` without a leading `+` when it is a plain decimal `Decimal()`
/// reads the same way as `DecimalNumber::parse`: an optional sign, ASCII
/// digits with an optional point, and an optional exponent.
pub fn decimal_text(text: &str) -> Option<&str> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if integer.len() + fraction.len() == 0 || !digits(integer) || !digits(fraction) {
        return None;
    }
    if let Some(exponent) = exponent {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if exponent.is_empty() || !digits(exponent) {
            return None;
        }
    }
    Some(text.strip_prefix('+').unwrap_or(text))
}

/// `floatformat`'s argument: the precision and the `g`/`u` suffix options.
/// `None` leaves the argument to Django.
fn floatformat_argument(argument: &ResolvedArgument<'_, '_>) -> Option<(i64, NumberOptions)> {
    let value = match argument {
        ResolvedArgument::Absent => return Some((-1, NumberOptions::default())),
        ResolvedArgument::Present(value) => value.as_ref()?,
    };
    let text = match value {
        Content::String(text) => text.as_raw().as_ref(),
        Content::Py(object)
            if object.is_exact_instance_of::<PyString>()
                || is_exact_safe_string(object).unwrap_or(false) =>
        {
            object.cast::<PyString>().ok()?.to_str().ok()?
        }
        other => return Some((integer_argument(other)?, NumberOptions::default())),
    };
    let mut options = NumberOptions::default();
    let precision = if let Some(rest) = text.strip_suffix("gu").or_else(|| text.strip_suffix("ug"))
    {
        options.force_grouping = true;
        options.localize = false;
        rest
    } else if let Some(rest) = text.strip_suffix('g') {
        options.force_grouping = true;
        rest
    } else if let Some(rest) = text.strip_suffix('u') {
        options.localize = false;
        rest
    } else if text.is_empty() {
        // Django's `arg[-1]` raises `IndexError`.
        return None;
    } else {
        return Some((python_int(text)?, options));
    };
    // `arg[:-1] or -1`.
    let precision = if precision.is_empty() {
        -1
    } else {
        python_int(precision)?
    };
    Some((precision, options))
}

pub fn floatformat(
    value: &Content<'_, '_>,
    precision: i64,
    options: NumberOptions,
    py: Python<'_>,
    context: &mut Context,
) -> PyResult<Option<String>> {
    let text = match value {
        // `Decimal(str(value))` for plain decimal strings; others, such as
        // `" 1"`, `"1_0"` and `"NaN"`, are left to Django.
        Content::String(text) => match decimal_text(text.as_raw()) {
            Some(text) => text.to_string(),
            None => return Ok(None),
        },
        Content::Py(object)
            if object.is_exact_instance_of::<PyString>()
                || is_exact_safe_string(object).unwrap_or(false) =>
        {
            match decimal_text(object.cast::<PyString>()?.to_str()?) {
                Some(text) => text.to_string(),
                None => return Ok(None),
            }
        }
        // `Decimal("True")` and `float("True")` fail, so Django returns "".
        Content::Bool(_) => return Ok(Some(String::new())),
        Content::Int(n) => n.to_string(),
        Content::Float(n) => PyFloat::new(py, *n).str()?.to_string(),
        Content::Py(object)
            if object.is_exact_instance_of::<PyInt>()
                || object.is_exact_instance_of::<PyFloat>()
                || object
                    .get_type()
                    .is(DECIMAL.import(py, "decimal", "Decimal")?) =>
        {
            match object.str() {
                Ok(text) => text.to_string(),
                Err(_) => return Ok(None),
            }
        }
        Content::Py(_) => return Ok(None),
    };
    let Some(number) = DecimalNumber::parse(&text) else {
        return Ok(None);
    };
    let places = usize::try_from(precision.unsigned_abs()).unwrap_or(usize::MAX);
    if places > FLOATFORMAT_MAX_DIGITS
        || usize::try_from(number.exponent.unsigned_abs())
            .ok()
            .and_then(|exponent| exponent.checked_add(number.digits.len()))
            .is_none_or(|size| size > FLOATFORMAT_MAX_DIGITS)
    {
        return Ok(None);
    }
    // Whole numbers drop their decimal places unless the precision is positive.
    let fraction_digits = usize::try_from(-number.exponent).unwrap_or(0);
    let whole = number
        .digits
        .iter()
        .rev()
        .take(fraction_digits)
        .all(|&digit| digit == 0);
    let places = if whole && precision <= 0 { 0 } else { places };
    let Some(rounded) = number.round(places) else {
        return Ok(None);
    };
    let Some(format) = number_format(py, context, options)? else {
        return Ok(None);
    };
    Ok(Some(format.format(&rounded)))
}

impl ResolveFilter for FloatFormatFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        if let Some((precision, options)) = floatformat_argument(&argument)
            && let Some(value) = &variable
            && let Some(formatted) = floatformat(value, precision, options, py, context)?
        {
            return Ok(Some(Content::String(ContentString::HtmlSafe(Cow::Owned(
                formatted,
            )))));
        }
        self.django.call(variable, argument, py, context)
    }
}

impl ResolveFilter for GetDigitFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        let argument = self.django.resolve_argument(py, template, context)?;
        // Django converts both with `int()`. A negative value can index its
        // minus sign, which raises, so those stay with Django.
        if let Some(position) = argument.value().and_then(integer_argument)
            && let Some(value) = variable.as_ref().and_then(integer_argument)
            && value >= 0
        {
            let digits = value.to_string();
            let digit = match usize::try_from(position) {
                // Django returns the converted value for a position below 1.
                Err(_) | Ok(0) => value,
                Ok(position) => digits
                    .len()
                    .checked_sub(position)
                    .map_or(0, |index| i64::from(digits.as_bytes()[index] - b'0')),
            };
            return Ok(Some(Content::Int(digit.into())));
        }
        self.django.call(variable, argument, py, context)
    }
}

static NGETTEXT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static GETTEXT_FUNCTION: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// The `filesizeformat` units above bytes, as Django's translatable strings.
const FILE_SIZE_UNITS: [&str; 5] = ["%s KB", "%s MB", "%s GB", "%s TB", "%s PB"];

/// A translated `"%s KB"`-style template.
#[derive(Clone, Debug)]
enum UnitTemplate {
    /// Split around its only `%s`.
    Split(String, String),
    /// Any other template, %-formatted by Python.
    Python(String),
}

/// `filesizeformat`'s translated strings for one language.
#[derive(Clone, Debug, Default)]
pub struct FileSizeStrings {
    /// `ngettext("%(size)d byte", "%(size)d bytes", n) % {"size": n}`.
    bytes: HashMap<u64, String>,
    units: [Option<UnitTemplate>; 5],
}

impl FileSizeStrings {
    fn bytes(&mut self, py: Python<'_>, count: u64) -> PyResult<&str> {
        Ok(match self.bytes.entry(count) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let size = PyDict::new(py);
                size.set_item("size", count)?;
                let text = NGETTEXT
                    .import(py, "django.utils.translation", "ngettext")?
                    .call1(("%(size)d byte", "%(size)d bytes", count))?
                    .rem(size)?;
                entry.insert(text.extract()?)
            }
        })
    }

    fn unit(&mut self, py: Python<'_>, index: usize, number: &str) -> PyResult<String> {
        let template = match &mut self.units[index] {
            Some(template) => template,
            slot @ None => {
                let text: String = GETTEXT_FUNCTION
                    .import(py, "django.utils.translation", "gettext")?
                    .call1((FILE_SIZE_UNITS[index],))?
                    .extract()?;
                slot.insert(match text.split_once("%s") {
                    Some((before, after)) if text.matches('%').count() == 1 => {
                        UnitTemplate::Split(before.to_string(), after.to_string())
                    }
                    _ => UnitTemplate::Python(text),
                })
            }
        };
        Ok(match template {
            UnitTemplate::Split(before, after) => format!("{before}{number}{after}"),
            UnitTemplate::Python(text) => PyString::new(py, text).rem(number)?.extract()?,
        })
    }
}

/// Python's `round(n / divisor, 1)` for `n < 2⁵³`, where the division is
/// exact: the nearest tenth, ties to even. Returns the integer part and the
/// tenths digit.
fn round_tenths(n: u64, divisor: u64) -> (u64, u64) {
    let scaled = n * 10;
    let mut tenths = scaled / divisor;
    let remainder = scaled % divisor;
    if remainder * 2 > divisor || (remainder * 2 == divisor && tenths % 2 == 1) {
        tenths += 1;
    }
    (tenths / 10, tenths % 10)
}

impl ResolveFilter for FilesizeformatFilter {
    fn resolve<'t, 'py>(
        &self,
        variable: Option<Content<'t, 'py>>,
        py: Python<'py>,
        template: TemplateString<'t>,
        context: &mut Context,
    ) -> ResolveResult<'t, 'py> {
        // Django converts with `int()`; failures give "0 bytes", left to it.
        // Above 2⁵³, `bytes / unit` isn't exact.
        let Some(bytes) = variable.as_ref().and_then(integer_argument) else {
            return self.django.resolve(variable, py, template, context);
        };
        let size = bytes.unsigned_abs();
        if size >= 1 << 53 {
            return self.django.resolve(variable, py, template, context);
        }
        let safe = match &variable {
            Some(Content::String(ContentString::HtmlSafe(_))) => true,
            Some(Content::Py(value)) => {
                value.is_instance(SAFEDATA.import(py, "django.utils.safestring", "SafeData")?)?
            }
            _ => false,
        };
        let text = if size < 1 << 10 {
            let Some(strings) = context
                .caches()
                .file_size_strings
                .get_or_try_init(py, |_| Ok(FileSizeStrings::default()))?
            else {
                return self.django.resolve(variable, py, template, context);
            };
            strings.bytes(py, size)?.to_string()
        } else {
            let index = (63 - size.leading_zeros() as usize) / 10 - 1;
            let index = index.min(FILE_SIZE_UNITS.len() - 1);
            let (integer, tenth) = round_tenths(size, 1 << (10 * (index + 1)));
            // `formats.number_format(value, 1)`, localized like `floatformat`.
            let Some(format) = number_format(py, context, NumberOptions::default())? else {
                return self.django.resolve(variable, py, template, context);
            };
            let number = format.format(&Rounded {
                sign: Sign::Plus,
                integer: integer.to_string(),
                fraction: tenth.to_string(),
            });
            let Some(strings) = context
                .caches()
                .file_size_strings
                .get_or_try_init(py, |_| Ok(FileSizeStrings::default()))?
            else {
                return self.django.resolve(variable, py, template, context);
            };
            strings.unit(py, index, &number)?
        };
        // `"-%s" % value` for negative sizes, then `avoid_wrapping()`.
        let sign = if bytes < 0 { "-" } else { "" };
        let text = format!("{sign}{}", text.replace(' ', "\u{a0}"));
        Ok(Some(Content::String(if safe {
            ContentString::HtmlSafe(Cow::Owned(text))
        } else {
            ContentString::HtmlUnsafe(Cow::Owned(text))
        })))
    }
}

/// Python's `str()` of a float. A float literal in a template, such as
/// `{{ 1.0|lower }}`, reaches string filters as an `f64`.
pub fn python_float_str(value: f64) -> String {
    if value.is_nan() {
        "nan".to_string()
    } else if value.is_infinite() {
        if value < 0.0 { "-inf" } else { "inf" }.to_string()
    } else {
        python_float_repr(value)
    }
}

/// Python's `repr()` of a finite float, as `json.dumps` writes it: the
/// shortest round-trip digits, in exponent form below 1e-4 and from 1e16.
pub fn python_float_repr(value: f64) -> String {
    // Rust's shortest digits break exact ties upward; Python rounds them to
    // even, as Rust does when formatting to that many digits.
    let shortest = format!("{value:e}");
    let precision = shortest
        .split_once('e')
        .map_or(0, |(mantissa, _)| {
            mantissa.bytes().filter(u8::is_ascii_digit).count()
        })
        .saturating_sub(1);
    let formatted = format!("{value:.precision$e}");
    let (mantissa, exponent) = formatted.split_once('e').unwrap_or((&formatted, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let (sign, mantissa) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |mantissa| ("-", mantissa));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let mut output = String::from(sign);
    if (-4..16).contains(&exponent) {
        if let Ok(point) = usize::try_from(exponent + 1)
            && point > 0
        {
            if digits.len() > point {
                output.push_str(&digits[..point]);
                output.push('.');
                output.push_str(&digits[point..]);
            } else {
                output.push_str(&digits);
                output.extend(std::iter::repeat_n('0', point - digits.len()));
                output.push_str(".0");
            }
        } else {
            output.push_str("0.");
            output.extend(std::iter::repeat_n(
                '0',
                exponent.unsigned_abs() as usize - 1,
            ));
            output.push_str(&digits);
        }
    } else {
        output.push_str(&digits[..1]);
        if digits.len() > 1 {
            output.push('.');
            output.push_str(&digits[1..]);
        }
        let sign = if exponent < 0 { '-' } else { '+' };
        output.push_str(&format!("e{sign}{:02}", exponent.unsigned_abs()));
    }
    output
}
