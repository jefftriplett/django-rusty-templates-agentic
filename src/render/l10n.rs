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

/// The values `number_format` passes to `django.utils.numberformat.format`.
#[derive(Clone, Debug)]
pub struct NumberFormat {
    use_thousand_separator: bool,
    decimal_separator: String,
    thousand_separator: String,
    /// `NUMBER_GROUPING` as group sizes from the right. An integer setting is
    /// a single size; `None` is an integer setting of 0, which disables
    /// grouping.
    grouping: Option<Vec<i64>>,
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
        let grouping = if grouping.is_exact_instance_of::<PyInt>() {
            match grouping.extract::<i64>() {
                Ok(0) => None,
                Ok(size) => Some(vec![size]),
                Err(_) => return Ok(None),
            }
        } else if grouping.is_exact_instance_of::<PyTuple>()
            || grouping.is_exact_instance_of::<PyList>()
        {
            let sizes = grouping
                .try_iter()?
                .map(|size| {
                    Ok(Some(size?)
                        .filter(|size| size.is_exact_instance_of::<PyInt>())
                        .and_then(|size| size.extract::<i64>().ok()))
                })
                .collect::<PyResult<Option<Vec<_>>>>()?;
            // Django fails on an empty sequence once it groups digits.
            match sizes {
                Some(sizes) if !sizes.is_empty() => Some(sizes),
                _ => return Ok(None),
            }
        } else {
            return Ok(None);
        };
        Ok(Some(Self {
            use_thousand_separator,
            decimal_separator: decimal_separator.extract()?,
            thousand_separator: thousand_separator.extract()?,
            grouping,
        }))
    }

    /// Group sizes when Django inserts thousand separators.
    fn grouping(&self) -> Option<&[i64]> {
        self.grouping
            .as_deref()
            .filter(|_| self.use_thousand_separator)
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
        match self.grouping() {
            Some(grouping) => {
                group_digits(integer, grouping, &self.thousand_separator, &mut output)
            }
            None => output.push_str(integer),
        }
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

/// `localize(value, use_l10n)` for exact ints, floats and bools. `None` for
/// other values and for floats Django formats as decimals.
pub fn localize_number(value: &Bound<'_, PyAny>, context: &Context) -> PyResult<Option<String>> {
    if value.is_exact_instance_of::<PyBool>() {
        let text = if value.is_truthy()? { "True" } else { "False" };
        return Ok(Some(text.to_owned()));
    }
    let is_int = value.is_exact_instance_of::<PyInt>();
    if !is_int && !value.is_exact_instance_of::<PyFloat>() {
        return Ok(None);
    }
    let number = match is_int.then(|| value.extract::<i64>()) {
        Some(Ok(number)) => number.to_string(),
        _ => value.str()?.extract()?,
    };
    if context.use_l10n == Some(false) {
        return Ok(Some(number));
    }
    let Some(format) = context.number_format(value.py())? else {
        return Ok(None);
    };
    if is_int && format.grouping().is_none() {
        return Ok(Some(number));
    }
    // Django formats floats with an exponent through Decimal.
    if number.contains('e') {
        return Ok(None);
    }
    Ok(Some(format.format(&number)))
}
