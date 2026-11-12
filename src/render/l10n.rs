//! Native `django.utils.formats.localize` for the numbers templates print most.
//!
//! Django reads `USE_THOUSAND_SEPARATOR` and the active language's separators
//! for every number it renders. They only change between renders or inside
//! `{% language %}`, so a render reads them once, on its first number, and
//! that read is served from a cache shared by all renders.

use html_escape::encode_quoted_attribute;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use std::sync::Mutex;

use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::{MutexExt, PyOnceLock};
use pyo3::types::{PyBool, PyFloat, PyInt, PyList, PyModule, PyString, PyTuple, PyType};

use super::language_cache::get_language;
use super::types::Context;

static GET_FORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static FORMATS: PyOnceLock<Py<PyModule>> = PyOnceLock::new();

/// Languages whose number formats are kept at once.
const CACHED_LANGUAGES: usize = 16;

/// Number formats read in earlier renders, valid while Django's
/// `formats._format_cache` is the same dict.
struct CachedFormats {
    django_cache: Py<PyAny>,
    entries: Vec<(String, bool, Option<NumberFormat>)>,
}

static NUMBER_FORMATS: Mutex<Option<CachedFormats>> = Mutex::new(None);

/// How Django groups the digits of a number's integer part.
#[derive(Clone, Debug)]
pub enum Grouping {
    /// No thousand separators: `USE_THOUSAND_SEPARATOR` is off or
    /// `NUMBER_GROUPING` is 0.
    Off,
    /// `NUMBER_GROUPING` as group sizes from the right. An integer setting is
    /// a single size.
    Sizes(Vec<i64>),
}

impl Grouping {
    /// Appends `digits`, inserting `separator` between groups.
    pub fn push_digits(&self, digits: &str, separator: &str, output: &mut String) {
        match self {
            Self::Off => output.push_str(digits),
            Self::Sizes(sizes) => group_digits(digits, sizes, separator, output),
        }
    }
}

/// The values `number_format` passes to `django.utils.numberformat.format`.
#[derive(Clone, Debug)]
pub struct NumberFormat {
    decimal_separator: String,
    thousand_separator: String,
    grouping: Grouping,
}

impl NumberFormat {
    /// The active language's formats, from the cache of earlier renders when
    /// Django's own format cache would answer too. `None` when a value has a
    /// type the native formatter does not reproduce, such as a lazy string:
    /// Django then formats every number of the render.
    pub fn load(py: Python<'_>) -> PyResult<Option<Self>> {
        let use_thousand_separator = SETTINGS
            .import(py, "django.conf", "settings")?
            .getattr(intern!(py, "USE_THOUSAND_SEPARATOR"))?
            .is_truthy()?;
        let language = get_language(py)?;
        let Ok(language) = language.cast_exact::<PyString>() else {
            return Self::read(py, use_thousand_separator);
        };
        let language = language.to_str()?;
        // Django keys its own cache by format name and language, and replaces
        // the dict when a format setting changes (`reset_format_cache`, which
        // `override_settings` calls). Entries made against an older dict are
        // dropped, so this cache is never staler than Django's.
        let django_cache = FORMATS
            .get_or_try_init(py, || py.import("django.utils.formats").map(Bound::unbind))?
            .bind(py)
            .getattr(intern!(py, "_format_cache"))?;
        {
            let cached = NUMBER_FORMATS
                .lock_py_attached(py)
                .expect("number format cache lock is not poisoned");
            if let Some(cached) = cached.as_ref()
                && cached.django_cache.bind(py).is(&django_cache)
                && let Some((.., format)) = cached.entries.iter().find(|(name, thousands, _)| {
                    name == language && *thousands == use_thousand_separator
                })
            {
                return Ok(format.clone());
            }
        }
        let format = Self::read(py, use_thousand_separator)?;
        let mut guard = NUMBER_FORMATS
            .lock_py_attached(py)
            .expect("number format cache lock is not poisoned");
        let cached = match guard.as_mut() {
            Some(cached) if cached.django_cache.bind(py).is(&django_cache) => cached,
            _ => guard.insert(CachedFormats {
                django_cache: django_cache.unbind(),
                entries: Vec::new(),
            }),
        };
        if cached.entries.len() >= CACHED_LANGUAGES {
            cached.entries.remove(0);
        }
        cached
            .entries
            .push((language.to_owned(), use_thousand_separator, format.clone()));
        drop(guard);
        Ok(format)
    }

    /// Read the active language's formats from Django.
    fn read(py: Python<'_>, use_thousand_separator: bool) -> PyResult<Option<Self>> {
        let get_format = GET_FORMAT.import(py, "django.utils.formats", "get_format")?;
        let get_format = |name| get_format.call1((name, py.None(), true));
        let decimal_separator = get_format("DECIMAL_SEPARATOR")?;
        let thousand_separator = get_format("THOUSAND_SEPARATOR")?;
        let grouping = get_format("NUMBER_GROUPING")?;
        let (Ok(decimal_separator), Ok(thousand_separator)) = (
            decimal_separator.cast_exact::<PyString>(),
            thousand_separator.cast_exact::<PyString>(),
        ) else {
            return Ok(None);
        };
        // Django only reads NUMBER_GROUPING when it inserts separators.
        let grouping = if !use_thousand_separator {
            Grouping::Off
        } else if grouping.is_exact_instance_of::<PyInt>() {
            match grouping.extract::<i64>() {
                Ok(0) => Grouping::Off,
                Ok(size) => Grouping::Sizes(vec![size]),
                Err(_) => return Ok(None),
            }
        } else if grouping.is_exact_instance_of::<PyTuple>()
            || grouping.is_exact_instance_of::<PyList>()
        {
            let sizes = grouping
                .try_iter()?
                .map(|size| {
                    Ok(Some(size?)
                        .filter(PyAnyMethods::is_exact_instance_of::<PyInt>)
                        .and_then(|size| size.extract::<i64>().ok()))
                })
                .collect::<PyResult<Option<Vec<_>>>>()?;
            // Django fails on an empty sequence once it groups digits.
            match sizes {
                Some(sizes) if !sizes.is_empty() => Grouping::Sizes(sizes),
                _ => return Ok(None),
            }
        } else {
            return Ok(None);
        };
        Ok(Some(Self {
            decimal_separator: decimal_separator.extract()?,
            thousand_separator: thousand_separator.extract()?,
            grouping,
        }))
    }

    /// Port of the string branch of `numberformat.format` without
    /// `decimal_pos`, which `localize` never passes.
    fn format(&self, number: &str) -> String {
        let (sign, number) = match number.strip_prefix('-') {
            Some(number) => ("-", number),
            None => ("", number),
        };
        let (integer, decimals) = match number.split_once('.') {
            Some((integer, decimals)) => (integer, Some(decimals)),
            None => (number, None),
        };
        let mut output = String::with_capacity(number.len() + 8);
        output.push_str(sign);
        self.grouping
            .push_digits(integer, &self.thousand_separator, &mut output);
        if let Some(decimals) = decimals.filter(|decimals| !decimals.is_empty()) {
            output.push_str(&self.decimal_separator);
            output.push_str(decimals);
        }
        output
    }
}

/// Django walks the digits from the right and inserts a separator each time a
/// group fills. A 0 size repeats the previous one and a negative size stops
/// grouping.
fn group_digits(digits: &str, grouping: &[i64], separator: &str, output: &mut String) {
    let mut sizes = grouping.iter().copied();
    let mut size = sizes.next().unwrap_or(0);
    let mut boundaries = Vec::new();
    let mut grouped = 0;
    while let Ok(step) = usize::try_from(size)
        && step > 0
    {
        grouped += step;
        if grouped >= digits.len() {
            break;
        }
        boundaries.push(digits.len() - grouped);
        if let Some(next) = sizes.next()
            && next != 0
        {
            size = next;
        }
    }
    let mut start = 0;
    for &boundary in boundaries.iter().rev() {
        output.push_str(&digits[start..boundary]);
        output.push_str(separator);
        start = boundary;
    }
    output.push_str(&digits[start..]);
}

impl Context {
    /// The formats for this render's active language, read on first use.
    fn number_format(&self, py: Python<'_>) -> PyResult<Option<&NumberFormat>> {
        if let Some(format) = self.number_format.get() {
            return Ok(format.as_ref());
        }
        let format = NumberFormat::load(py)?;
        Ok(self.number_format.get_or_init(|| format).as_ref())
    }
}

/// `localize(value)` for an integer Rusty computed itself, such as
/// `forloop.counter`, an integer literal or the result of `add`, escaped for
/// output when autoescaping. `None` when the active formats need Django.
pub fn localize_int(number: &BigInt, context: &Context) -> PyResult<Option<String>> {
    // `i64`'s formatting is much cheaper than `BigInt`'s.
    let digits = number
        .to_i64()
        .map_or_else(|| number.to_string(), |number| number.to_string());
    if context.use_l10n == Some(false) {
        return Ok(Some(digits));
    }
    // The formats are read once per render; only that first read needs Python.
    let format = match context.number_format.get() {
        Some(format) => format.as_ref(),
        None => Python::attach(|py| context.number_format(py))?,
    };
    Ok(format.map(|format| match format.grouping {
        // Plain digits never need escaping.
        Grouping::Off => digits,
        Grouping::Sizes(_) => {
            let number = format.format(&digits);
            // Separators such as `'` are escaped like Django's output.
            if context.autoescape && number.contains(['&', '<', '>', '"', '\'']) {
                encode_quoted_attribute(&number).into_owned()
            } else {
                number
            }
        }
    }))
}

/// The number types `localize_number` formats natively.
#[derive(Clone, Copy)]
enum Number {
    Int,
    Float,
    Decimal,
}

static DECIMAL: PyOnceLock<Py<PyType>> = PyOnceLock::new();

/// Whether `value` is exactly a `decimal.Decimal`.
fn is_decimal(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    let decimal = DECIMAL.import(value.py(), "decimal", "Decimal")?;
    Ok(value.get_type().is(decimal))
}

/// `localize(value, use_l10n)` for exact ints, floats, decimals and bools.
/// `None` for other values, for floats Django formats as decimals, and for
/// decimals it writes with an exponent or leaves unchanged when not finite.
pub fn localize_number(
    value: &Bound<'_, PyAny>,
    use_l10n: Option<bool>,
    context: &Context,
) -> PyResult<Option<String>> {
    if value.is_exact_instance_of::<PyBool>() {
        let text = if value.is_truthy()? { "True" } else { "False" };
        return Ok(Some(text.to_owned()));
    }
    let kind = if value.is_exact_instance_of::<PyInt>() {
        Number::Int
    } else if value.is_exact_instance_of::<PyFloat>() {
        Number::Float
    } else if is_decimal(value)? {
        Number::Decimal
    } else {
        return Ok(None);
    };
    let number = match kind {
        Number::Int => match value.extract::<i64>() {
            Ok(number) => number.to_string(),
            Err(_) => value.str()?.extract()?,
        },
        Number::Float | Number::Decimal => value.str()?.extract()?,
    };
    if use_l10n == Some(false) {
        return Ok(Some(number));
    }
    let Some(format) = context.number_format(value.py())? else {
        return Ok(None);
    };
    match kind {
        Number::Int if matches!(format.grouping, Grouping::Off) => Ok(Some(number)),
        // Django formats floats with an exponent through Decimal.
        Number::Float if number.contains('e') => Ok(None),
        // `str()` matches Django's `"{:f}"` unless it uses an exponent or the
        // value isn't finite. Django switches to `"{:e}"` past 200 digits,
        // which a string of 100 characters can't reach.
        Number::Decimal
            if number.len() > 100 || number.bytes().any(|byte| byte.is_ascii_alphabetic()) =>
        {
            Ok(None)
        }
        Number::Int | Number::Float | Number::Decimal => Ok(Some(format.format(&number))),
    }
}

/// Date format strings read in earlier renders, under the same rule as
/// [`NumberFormat::load`]: valid for one language while Django's
/// `formats._format_cache` is the same dict.
struct CachedDateFormats {
    django_cache: Py<PyAny>,
    entries: Vec<(String, String, Option<String>)>,
}

static DATE_FORMATS: Mutex<Option<CachedDateFormats>> = Mutex::new(None);

/// Most date format strings kept, across languages and format names.
const CACHED_DATE_FORMATS: usize = 64;

/// `get_format(name)` as a native string, or `None` when Django returns
/// something else, such as a lazy translation.
pub fn date_format(py: Python<'_>, name: &str) -> PyResult<Option<String>> {
    let read = || -> PyResult<Option<String>> {
        let format = GET_FORMAT
            .import(py, "django.utils.formats", "get_format")?
            .call1((name,))?;
        Ok(match format.cast_exact::<PyString>() {
            Ok(format) => Some(format.to_cow()?.into_owned()),
            Err(_) => None,
        })
    };
    let language = get_language(py)?;
    let Ok(language) = language.cast_exact::<PyString>() else {
        return read();
    };
    let language = language.to_str()?;
    let django_cache = FORMATS
        .get_or_try_init(py, || py.import("django.utils.formats").map(Bound::unbind))?
        .bind(py)
        .getattr(intern!(py, "_format_cache"))?;
    let lock = || {
        DATE_FORMATS
            .lock_py_attached(py)
            .expect("date format cache lock is not poisoned")
    };
    {
        let cached = lock();
        if let Some(cached) = cached.as_ref()
            && cached.django_cache.bind(py).is(&django_cache)
            && let Some((.., format)) =
                cached
                    .entries
                    .iter()
                    .find(|(cached_language, cached_name, _)| {
                        cached_language == language && cached_name == name
                    })
        {
            return Ok(format.clone());
        }
    }
    let format = read()?;
    let mut guard = lock();
    let cached = match guard.as_mut() {
        Some(cached) if cached.django_cache.bind(py).is(&django_cache) => cached,
        _ => guard.insert(CachedDateFormats {
            django_cache: django_cache.unbind(),
            entries: Vec::new(),
        }),
    };
    if cached.entries.len() >= CACHED_DATE_FORMATS {
        cached.entries.remove(0);
    }
    cached
        .entries
        .push((language.to_owned(), name.to_owned(), format.clone()));
    drop(guard);
    Ok(format)
}
