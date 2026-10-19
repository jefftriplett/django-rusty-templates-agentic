//! Native `django.utils.formats.localize` for the numbers templates print most.
//!
//! Django reads `USE_THOUSAND_SEPARATOR` and the active language's separators
//! for every number it renders. They only change between renders or inside
//! `{% language %}`, so a render reads them once, on its first number.

use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyFloat, PyInt, PyList, PyString, PyTuple};

use super::types::Context;

static GET_FORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

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
    /// Read the active language's formats. `None` when a value has a type the
    /// native formatter does not reproduce, such as a lazy string: Django then
    /// formats every number of the render.
    pub fn load(py: Python<'_>) -> PyResult<Option<Self>> {
        let use_thousand_separator = SETTINGS
            .import(py, "django.conf", "settings")?
            .getattr("USE_THOUSAND_SEPARATOR")?
            .is_truthy()?;
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

/// The number types `localize_number` formats natively.
#[derive(Clone, Copy)]
enum Number {
    Int,
    Float,
}

/// `localize(value, use_l10n)` for exact ints, floats and bools. `None` for
/// other values and for floats Django formats as decimals.
pub fn localize_number(value: &Bound<'_, PyAny>, context: &Context) -> PyResult<Option<String>> {
    if value.is_exact_instance_of::<PyBool>() {
        let text = if value.is_truthy()? { "True" } else { "False" };
        return Ok(Some(text.to_owned()));
    }
    let kind = if value.is_exact_instance_of::<PyInt>() {
        Number::Int
    } else if value.is_exact_instance_of::<PyFloat>() {
        Number::Float
    } else {
        return Ok(None);
    };
    let number = match kind {
        Number::Int => match value.extract::<i64>() {
            Ok(number) => number.to_string(),
            Err(_) => value.str()?.extract()?,
        },
        Number::Float => value.str()?.extract()?,
    };
    if context.use_l10n == Some(false) {
        return Ok(Some(number));
    }
    let Some(format) = context.number_format(value.py())? else {
        return Ok(None);
    };
    match kind {
        Number::Int if matches!(format.grouping, Grouping::Off) => Ok(Some(number)),
        // Django formats floats with an exponent through Decimal.
        Number::Float if number.contains('e') => Ok(None),
        Number::Int | Number::Float => Ok(Some(format.format(&number))),
    }
}
