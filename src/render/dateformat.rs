//! Native port of `django.utils.dateformat` for the `date` and `time` filters.
//!
//! Django looks up the active language's format strings, month and weekday
//! names and the current timezone for every value it formats. Within a render
//! those only change inside `{% language %}` and `{% timezone %}`, so
//! [`DateCache`] reads each one once and those tags give their body a fresh
//! cache.
//!
//! Anything else is left to Django. Subclasses of the `datetime` types and
//! tzinfo implementations other than `datetime.timezone` and
//! `zoneinfo.ZoneInfo` go to the filter itself. The timezone specifiers
//! (`e I O r T U Z`) and specifiers the value lacks go to
//! `django.utils.dateformat` with the already resolved format.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Write;

use pyo3::exceptions::{PyAttributeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{
    PyDate, PyDateAccess, PyDateTime, PyString, PyTime, PyTimeAccess, PyType, PyTzInfoAccess,
};
use pyo3::{PyTraverseError, PyVisit};

use crate::gc::Trace;
use crate::render::common::gettext;

static GET_FORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DATEFORMAT_FORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static DATEFORMAT_TIME_FORMAT: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static GET_CURRENT_TIMEZONE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static TEMPLATE_LOCALTIME: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static SETTINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static FIXED_OFFSET: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static ZONE_INFO: PyOnceLock<Py<PyType>> = PyOnceLock::new();

/// The characters `django.utils.dateformat.re_formatchars` matches.
const FORMAT_CHARS: &str = "aAbcdDeEfFgGhHiIjlLmMnNoOPrsStTUuwWyYzZ";
/// The specifiers `TimeFormat` defines. `DateFormat` adds the rest.
const TIME_CHARS: &str = "aAefgGhHiOPsTuZ";

/// Which of Django's formatter classes to mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Formatter {
    /// `DateFormat`, used by the `date` filter.
    Date,
    /// `TimeFormat`, used by the `time` filter.
    Time,
}

impl Formatter {
    /// The format setting the filter uses when its argument is falsy.
    pub fn default_format(self) -> &'static str {
        match self {
            Self::Date => "DATE_FORMAT",
            Self::Time => "TIME_FORMAT",
        }
    }
}

/// A translated string from `django.utils.dates` or a meridiem from gettext.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Name {
    Month(u8),
    MonthAbbr(u8),
    MonthAbbrTitle(u8),
    MonthAlt(u8),
    MonthAp(u8),
    Weekday(u8),
    WeekdayAbbr(u8),
    Meridiem(&'static str),
}

impl Name {
    fn fetch(self, py: Python<'_>) -> PyResult<String> {
        let (table, key) = match self {
            Self::Month(month) => ("MONTHS", month),
            Self::MonthAbbr(month) | Self::MonthAbbrTitle(month) => ("MONTHS_3", month),
            Self::MonthAlt(month) => ("MONTHS_ALT", month),
            Self::MonthAp(month) => ("MONTHS_AP", month),
            Self::Weekday(day) => ("WEEKDAYS", day),
            Self::WeekdayAbbr(day) => ("WEEKDAYS_ABBR", day),
            Self::Meridiem(text) => return gettext(py, text),
        };
        let name = py
            .import("django.utils.dates")?
            .getattr(table)?
            .get_item(key)?;
        let name = match self {
            Self::MonthAbbrTitle(_) => name.call_method0("title")?,
            _ => name,
        };
        name.str()?.extract()
    }
}

#[derive(Debug, Default)]
struct Names(HashMap<Name, String>);

impl Names {
    fn get(&mut self, py: Python<'_>, name: Name) -> PyResult<&str> {
        Ok(match self.0.entry(name) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(name.fetch(py)?),
        })
    }
}

/// Per-render snapshot of the localization state the formatters read.
#[derive(Debug, Default)]
pub struct DateCache {
    /// `get_format(name)` results; `None` when Django returned a non-string.
    formats: HashMap<String, Option<String>>,
    names: Names,
    timezone: Option<Py<PyAny>>,
}

impl Trace for DateCache {
    // Format strings and names are native copies; only the timezone is Python.
    fn trace(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.timezone.trace(visit)
    }
}

impl DateCache {
    fn current_timezone<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if let Some(timezone) = &self.timezone {
            return Ok(timezone.bind(py).clone());
        }
        let timezone = GET_CURRENT_TIMEZONE
            .import(py, "django.utils.timezone", "get_current_timezone")?
            .call0()?;
        self.timezone = Some(timezone.clone().unbind());
        Ok(timezone)
    }

    /// `formats.date_format(value, format_name)`, or `formats.time_format`
    /// for [`Formatter::Time`]. `None` where Django's filter must handle the
    /// value: other value types, a non-string format setting, or an error the
    /// filter catches before retrying with the unresolved argument.
    pub fn format<'py>(
        &mut self,
        value: &Bound<'py, PyAny>,
        format_name: &str,
        formatter: Formatter,
    ) -> PyResult<Option<Formatted<'py>>> {
        let py = value.py();
        let Some(fields) = fields(value)? else {
            return Ok(None);
        };
        if !self.formats.contains_key(format_name) {
            let format = GET_FORMAT
                .import(py, "django.utils.formats", "get_format")?
                .call1((format_name,))?;
            let format = match format.cast_exact::<PyString>() {
                Ok(format) => Some(format.to_cow()?.into_owned()),
                Err(_) => None,
            };
            self.formats.insert(format_name.to_string(), format);
        }
        let Some(format) = &self.formats[format_name] else {
            return Ok(None);
        };
        if let Some(formatted) = format_value(fields, value, format, formatter, &mut self.names)? {
            return Ok(Some(Formatted::Native(formatted)));
        }
        let django_format = match formatter {
            Formatter::Date => DATEFORMAT_FORMAT.import(py, "django.utils.dateformat", "format")?,
            Formatter::Time => {
                DATEFORMAT_TIME_FORMAT.import(py, "django.utils.dateformat", "time_format")?
            }
        };
        match django_format.call1((value, format)) {
            Ok(formatted) => Ok(Some(Formatted::Django(formatted))),
            Err(error)
                if error.is_instance_of::<PyAttributeError>(py)
                    || (formatter == Formatter::Time
                        && error.is_instance_of::<PyTypeError>(py)) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

/// A value formatted natively or by `django.utils.dateformat`.
pub enum Formatted<'py> {
    Native(String),
    Django(Bound<'py, PyAny>),
}

/// `django.utils.timezone.template_localtime`, applied by Django to the input
/// of filters registered with `expects_localtime=True`.
pub fn template_localtime<'py>(
    value: Bound<'py, PyAny>,
    use_tz: Option<bool>,
    cache: &mut DateCache,
) -> PyResult<Bound<'py, PyAny>> {
    let py = value.py();
    let Ok(datetime) = value.cast_exact::<PyDateTime>() else {
        if value.is_instance_of::<PyDateTime>() {
            // Subclasses may define `convert_to_local_time`.
            return TEMPLATE_LOCALTIME
                .import(py, "django.utils.timezone", "template_localtime")?
                .call1((value, use_tz));
        }
        return Ok(value);
    };
    if datetime.get_tzinfo().is_none() {
        return Ok(value);
    }
    let use_tz = match use_tz {
        Some(use_tz) => use_tz,
        None => SETTINGS
            .import(py, "django.conf", "settings")?
            .getattr(pyo3::intern!(py, "USE_TZ"))?
            .is_truthy()?,
    };
    if !use_tz
        || value
            .call_method0(pyo3::intern!(py, "utcoffset"))?
            .is_none()
    {
        return Ok(value);
    }
    value.call_method1(
        pyo3::intern!(py, "astimezone"),
        (cache.current_timezone(py)?,),
    )
}

#[derive(Clone, Copy)]
struct Date {
    year: i32,
    month: u8,
    day: u8,
}

#[derive(Clone, Copy)]
struct Time {
    hour: u8,
    minute: u8,
    second: u8,
    microsecond: u32,
}

impl Time {
    fn hour12(self) -> u8 {
        match self.hour % 12 {
            0 => 12,
            hour => hour,
        }
    }
}

/// Splits exact `date`, `datetime` and `time` values into their fields.
/// `None` for anything that might not behave like the built-in types.
fn fields(value: &Bound<'_, PyAny>) -> PyResult<Option<(Option<Date>, Option<Time>)>> {
    let py = value.py();
    if let Ok(datetime) = value.cast_exact::<PyDateTime>() {
        // `TimeFormat.__init__` calls `utcoffset()` on any tzinfo, which other
        // implementations may not support.
        if let Some(tzinfo) = datetime.get_tzinfo() {
            let tzinfo_type = tzinfo.get_type();
            if !tzinfo_type.is(FIXED_OFFSET.import(py, "datetime", "timezone")?)
                && !tzinfo_type.is(ZONE_INFO.import(py, "zoneinfo", "ZoneInfo")?)
            {
                return Ok(None);
            }
        }
        let date = Date {
            year: datetime.get_year(),
            month: datetime.get_month(),
            day: datetime.get_day(),
        };
        let time = Time {
            hour: datetime.get_hour(),
            minute: datetime.get_minute(),
            second: datetime.get_second(),
            microsecond: datetime.get_microsecond(),
        };
        Ok(Some((Some(date), Some(time))))
    } else if let Ok(date) = value.cast_exact::<PyDate>() {
        let date = Date {
            year: date.get_year(),
            month: date.get_month(),
            day: date.get_day(),
        };
        Ok(Some((Some(date), None)))
    } else if let Ok(time) = value.cast_exact::<PyTime>() {
        let time = Time {
            hour: time.get_hour(),
            minute: time.get_minute(),
            second: time.get_second(),
            microsecond: time.get_microsecond(),
        };
        Ok(Some((None, Some(time))))
    } else {
        Ok(None)
    }
}

/// `Formatter.format`: specifiers not preceded by a backslash are replaced
/// and `\x` in the remaining text becomes `x` (except before a newline, which
/// the `.` in `re_escaped` does not match).
fn format_value(
    (date, time): (Option<Date>, Option<Time>),
    value: &Bound<'_, PyAny>,
    format: &str,
    formatter: Formatter,
    names: &mut Names,
) -> PyResult<Option<String>> {
    let mut output = String::with_capacity(format.len() * 2);
    let mut chars = format.chars().peekable();
    let mut previous = None;
    while let Some(character) = chars.next() {
        let escaped = previous == Some('\\');
        previous = Some(character);
        if escaped || !FORMAT_CHARS.contains(character) {
            if character == '\\'
                && let Some(next) = chars.next_if(|next| *next != '\n')
            {
                output.push(next);
                previous = Some(next);
            } else {
                output.push(character);
            }
            continue;
        }
        let handled = if TIME_CHARS.contains(character) {
            // Django raises TypeError for time specifiers on a plain date.
            match time {
                Some(time) => push_time_specifier(character, time, names, &mut output, value)?,
                None => false,
            }
        } else {
            // `TimeFormat` and `time` values have no date specifiers.
            match (formatter, date) {
                (Formatter::Date, Some(date)) => {
                    push_date_specifier(character, date, names, &mut output, value)?
                }
                _ => false,
            }
        };
        if !handled {
            return Ok(None);
        }
    }
    Ok(Some(output))
}

/// Appends one `TimeFormat` specifier. `false` leaves it to Django.
fn push_time_specifier(
    specifier: char,
    time: Time,
    names: &mut Names,
    output: &mut String,
    value: &Bound<'_, PyAny>,
) -> PyResult<bool> {
    let py = value.py();
    let pm = time.hour > 11;
    match specifier {
        'a' => output.push_str(names.get(py, Name::Meridiem(if pm { "p.m." } else { "a.m." }))?),
        'A' => output.push_str(names.get(py, Name::Meridiem(if pm { "PM" } else { "AM" }))?),
        'f' => push_hour_minute(time, output),
        'g' => push_fmt(output, format_args!("{}", time.hour12())),
        'G' => push_fmt(output, format_args!("{}", time.hour)),
        'h' => push_fmt(output, format_args!("{:02}", time.hour12())),
        'H' => push_fmt(output, format_args!("{:02}", time.hour)),
        'i' => push_fmt(output, format_args!("{:02}", time.minute)),
        'P' => match (time.hour, time.minute) {
            (0, 0) => output.push_str(names.get(py, Name::Meridiem("midnight"))?),
            (12, 0) => output.push_str(names.get(py, Name::Meridiem("noon"))?),
            _ => {
                push_hour_minute(time, output);
                output.push(' ');
                output.push_str(names.get(py, Name::Meridiem(if pm { "p.m." } else { "a.m." }))?);
            }
        },
        's' => push_fmt(output, format_args!("{:02}", time.second)),
        'u' => push_fmt(output, format_args!("{:06}", time.microsecond)),
        // e, O, T, Z depend on the timezone.
        _ => return Ok(false),
    }
    Ok(true)
}

/// `TimeFormat.f`: 12-hour time with the minutes left off when zero.
fn push_hour_minute(time: Time, output: &mut String) {
    if time.minute == 0 {
        push_fmt(output, format_args!("{}", time.hour12()));
    } else {
        push_fmt(output, format_args!("{}:{:02}", time.hour12(), time.minute));
    }
}

/// Appends formatted text without an intermediate allocation.
fn push_fmt(output: &mut String, arguments: std::fmt::Arguments<'_>) {
    output
        .write_fmt(arguments)
        .expect("writing to a String cannot fail");
}

/// Appends one `DateFormat` specifier. `false` leaves it to Django.
fn push_date_specifier(
    specifier: char,
    date: Date,
    names: &mut Names,
    output: &mut String,
    value: &Bound<'_, PyAny>,
) -> PyResult<bool> {
    let py = value.py();
    let Date { year, month, day } = date;
    match specifier {
        'b' => output.push_str(names.get(py, Name::MonthAbbr(month))?),
        'c' => output.push_str(&value.call_method0("isoformat")?.extract::<String>()?),
        'd' => push_fmt(output, format_args!("{day:02}")),
        'D' => output.push_str(names.get(py, Name::WeekdayAbbr(weekday(date)))?),
        'E' => output.push_str(names.get(py, Name::MonthAlt(month))?),
        'F' => output.push_str(names.get(py, Name::Month(month))?),
        'j' => push_fmt(output, format_args!("{day}")),
        'l' => output.push_str(names.get(py, Name::Weekday(weekday(date)))?),
        'L' => output.push_str(if is_leap(year) { "True" } else { "False" }),
        'm' => push_fmt(output, format_args!("{month:02}")),
        'M' => output.push_str(names.get(py, Name::MonthAbbrTitle(month))?),
        'n' => push_fmt(output, format_args!("{month}")),
        'N' => output.push_str(names.get(py, Name::MonthAp(month))?),
        'o' => push_fmt(output, format_args!("{}", iso_week(date).0)),
        'S' => output.push_str(match day {
            11..=13 => "th",
            _ => match day % 10 {
                1 => "st",
                2 => "nd",
                3 => "rd",
                _ => "th",
            },
        }),
        't' => push_fmt(output, format_args!("{}", days_in_month(year, month))),
        'w' => push_fmt(output, format_args!("{}", (weekday(date) + 1) % 7)),
        'W' => push_fmt(output, format_args!("{}", iso_week(date).1)),
        'y' => push_fmt(output, format_args!("{:02}", year % 100)),
        'Y' => push_fmt(output, format_args!("{year:04}")),
        'z' => push_fmt(output, format_args!("{}", day_of_year(date))),
        // I, r, U depend on the timezone.
        _ => return Ok(false),
    }
    Ok(true)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn day_of_year(date: Date) -> u16 {
    (1..date.month)
        .map(|month| u16::from(days_in_month(date.year, month)))
        .sum::<u16>()
        + u16::from(date.day)
}

/// `date.weekday()`: Monday is 0.
fn weekday(date: Date) -> u8 {
    // Sakamoto's method, which counts from Sunday.
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let year = if date.month < 3 {
        date.year - 1
    } else {
        date.year
    };
    let sunday_based = (year + year / 4 - year / 100
        + year / 400
        + OFFSETS[usize::from(date.month - 1)]
        + i32::from(date.day))
        % 7;
    u8::try_from((sunday_based + 6) % 7).expect("a weekday is between 0 and 6")
}

/// `date.isocalendar()[:2]`: the ISO year and week number.
fn iso_week(date: Date) -> (i32, u8) {
    let weeks_in = |year: i32| {
        let january_first = weekday(Date {
            year,
            month: 1,
            day: 1,
        });
        // Years starting on a Thursday, or leap years starting on a
        // Wednesday, have 53 weeks.
        if january_first == 3 || (january_first == 2 && is_leap(year)) {
            53
        } else {
            52
        }
    };
    let week = (i32::from(day_of_year(date)) - i32::from(weekday(date)) + 9) / 7;
    if week < 1 {
        (date.year - 1, weeks_in(date.year - 1))
    } else if week > i32::from(weeks_in(date.year)) {
        (date.year + 1, 1)
    } else {
        (
            date.year,
            u8::try_from(week).expect("an ISO week is between 1 and 53"),
        )
    }
}
