//! Native port of `django.utils.timesince` for the `timesince` and
//! `timeuntil` filters.
//!
//! Django formats every unit through lazy `ngettext` strings, which look up
//! the active translation on each call. [`TimeStrings`] keeps each formatted
//! unit for the active language, still produced by Django's `TIME_STRINGS`,
//! so the catalogs stay the source of the text.
//!
//! Only exact `date` and `datetime` values are computed here. A value is
//! aware when its `utcoffset()` isn't `None`, as Django's `is_aware` checks.
//! Python subtracts two datetimes by their wall clocks when they share a
//! tzinfo object, which Django's conversions usually give; when they don't,
//! the values are left to Django. Time zone conversion and the current time
//! still come from Python.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{
    PyDate, PyDateAccess, PyDateTime, PyDict, PyTimeAccess, PyType, PyTzInfo, PyTzInfoAccess,
};

use crate::render::common::gettext;

static DATETIME: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static FIXED_OFFSET: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static ZONE_INFO: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static TIME_STRINGS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static AVOID_WRAPPING: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// `TIME_CHUNKS` in microseconds: a week, a day, an hour and a minute.
const CHUNKS: [i64; 4] = [
    7 * 24 * 60 * 60 * 1_000_000,
    24 * 60 * 60 * 1_000_000,
    60 * 60 * 1_000_000,
    60 * 1_000_000,
];

/// `MONTHS_DAYS`, which gives February 28 days in every year.
const MONTHS_DAYS: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// The filters always use `timesince`'s default depth.
const DEPTH: usize = 2;

/// The keys of `TIME_STRINGS`, in the order of [`Partials`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Unit {
    Year,
    Month,
    Week,
    Day,
    Hour,
    Minute,
}

impl Unit {
    const ALL: [Self; 6] = [
        Self::Year,
        Self::Month,
        Self::Week,
        Self::Day,
        Self::Hour,
        Self::Minute,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Year => "year",
            Self::Month => "month",
            Self::Week => "week",
            Self::Day => "day",
            Self::Hour => "hour",
            Self::Minute => "minute",
        }
    }
}

/// Years, months, weeks, days, hours and minutes, in [`Unit::ALL`] order.
type Partials = [i64; 6];

/// Formatted units and the list separator of one language.
#[derive(Clone, Debug, Default)]
pub struct TimeStrings {
    /// `avoid_wrapping(TIME_STRINGS[unit] % {"num": count})`.
    units: HashMap<(Unit, i64), String>,
    /// `gettext(", ")`.
    separator: Option<String>,
}

impl TimeStrings {
    fn unit(&mut self, py: Python<'_>, unit: Unit, count: i64) -> PyResult<&str> {
        Ok(match self.units.entry((unit, count)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let num = PyDict::new(py);
                num.set_item("num", count)?;
                let text = TIME_STRINGS
                    .import(py, "django.utils.timesince", "TIME_STRINGS")?
                    .get_item(unit.key())?
                    .rem(num)?;
                entry.insert(
                    AVOID_WRAPPING
                        .import(py, "django.utils.html", "avoid_wrapping")?
                        .call1((text,))?
                        .extract()?,
                )
            }
        })
    }

    fn separator(&mut self, py: Python<'_>) -> PyResult<&str> {
        if self.separator.is_none() {
            self.separator = Some(gettext(py, ", ")?);
        }
        Ok(self.separator.as_deref().unwrap_or_default())
    }

    /// The text `timesince` builds from `partials`, or "0 minutes" for `None`.
    pub fn format(&mut self, py: Python<'_>, partials: Option<Partials>) -> PyResult<String> {
        let Some(partials) = partials else {
            return Ok(self.unit(py, Unit::Minute, 0)?.to_owned());
        };
        let mut result = String::new();
        let units = Unit::ALL.into_iter().zip(partials);
        for (depth, (unit, count)) in units
            .skip_while(|&(_, count)| count == 0)
            .take_while(|&(_, count)| count != 0)
            .take(DEPTH)
            .enumerate()
        {
            if depth > 0 {
                result.push_str(self.separator(py)?);
            }
            result.push_str(self.unit(py, unit, count)?);
        }
        Ok(result)
    }
}

/// The wall clock reading of a naive or aware datetime, ordered in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WallClock {
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    microsecond: u32,
}

impl WallClock {
    fn of(datetime: &Bound<'_, PyDateTime>) -> Self {
        Self {
            year: datetime.get_year(),
            month: datetime.get_month(),
            day: datetime.get_day(),
            hour: datetime.get_hour(),
            minute: datetime.get_minute(),
            second: datetime.get_second(),
            microsecond: datetime.get_microsecond(),
        }
    }

    fn time(self) -> (u8, u8, u8, u32) {
        (self.hour, self.minute, self.second, self.microsecond)
    }

    /// Microseconds since 0001-01-01, as Python's naive subtraction counts.
    fn microseconds(self) -> i64 {
        let days = days_from_civil(self.year, self.month, self.day);
        let seconds =
            (i64::from(self.hour) * 60 + i64::from(self.minute)) * 60 + i64::from(self.second);
        (days * 86_400 + seconds) * 1_000_000 + i64::from(self.microsecond)
    }
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

/// A `timesince` operand converted to a `datetime` as Django does.
struct Operand<'py> {
    wall_clock: WallClock,
    /// The value itself when it is an aware `datetime`.
    aware: Option<Bound<'py, PyDateTime>>,
}

impl<'py> Operand<'py> {
    /// `None` for values that may not behave like the built-in types.
    fn of(value: &Bound<'py, PyAny>) -> PyResult<Option<Self>> {
        let py = value.py();
        if let Ok(datetime) = value.cast_exact::<PyDateTime>() {
            let aware = match datetime.get_tzinfo() {
                None => None,
                Some(tzinfo) => {
                    // Both built-in tzinfo types always return an offset.
                    let tzinfo_type = tzinfo.get_type();
                    let known = tzinfo_type.is(FIXED_OFFSET.import(py, "datetime", "timezone")?)
                        || tzinfo_type.is(ZONE_INFO.import(py, "zoneinfo", "ZoneInfo")?);
                    if known || !datetime.call_method0("utcoffset")?.is_none() {
                        Some(datetime.clone())
                    } else {
                        None
                    }
                }
            };
            return Ok(Some(Self {
                wall_clock: WallClock::of(datetime),
                aware,
            }));
        }
        if let Ok(date) = value.cast_exact::<PyDate>() {
            return Ok(Some(Self {
                wall_clock: WallClock {
                    year: date.get_year(),
                    month: date.get_month(),
                    day: date.get_day(),
                    hour: 0,
                    minute: 0,
                    second: 0,
                    microsecond: 0,
                },
                aware: None,
            }));
        }
        Ok(None)
    }
}

/// What `timesince(d, now, reversed)` computes.
pub enum TimeSince {
    /// The units to show; `None` for "0 minutes".
    Partials(Option<Partials>),
    /// A naive and an aware value, whose subtraction raises the `TypeError`
    /// the filter turns into "".
    Mismatched,
}

/// `timesince(d, now, reversed)` for operands [`Operand::of`] accepts, with a
/// falsy `now` passed as `None`. `None` when either operand is left to Django.
pub fn timesince(
    d: &Bound<'_, PyAny>,
    now: Option<&Bound<'_, PyAny>>,
    reversed: bool,
) -> PyResult<Option<TimeSince>> {
    let py = d.py();
    let Some(d) = Operand::of(d)? else {
        return Ok(None);
    };
    let now = match now {
        None => {
            let datetime_now = DATETIME.import(py, "datetime", "datetime")?;
            let tzinfo = d.aware.as_ref().and_then(PyTzInfoAccess::get_tzinfo);
            let now = datetime_now.call_method1("now", (&tzinfo,))?;
            let now = now.cast::<PyDateTime>()?;
            if !same_tzinfo(tzinfo.as_ref(), now) {
                return Ok(None);
            }
            WallClock::of(now)
        }
        Some(now) => {
            let Some(now) = Operand::of(now)? else {
                return Ok(None);
            };
            match (&d.aware, &now.aware) {
                (None, None) => now.wall_clock,
                (Some(d), Some(now)) => {
                    let tzinfo = d.get_tzinfo();
                    let now = now.call_method1("astimezone", (&tzinfo,))?;
                    let now = now.cast::<PyDateTime>()?;
                    if !same_tzinfo(tzinfo.as_ref(), now) {
                        return Ok(None);
                    }
                    WallClock::of(now)
                }
                _ => return Ok(Some(TimeSince::Mismatched)),
            }
        }
    };
    let (d, now) = match reversed {
        false => (d.wall_clock, now),
        true => (now, d.wall_clock),
    };
    Ok(Some(TimeSince::Partials(partials(d, now))))
}

/// Whether `now` has the tzinfo object `tzinfo`, so that Python subtracts
/// it from a value with that tzinfo by wall clock. A conversion can return
/// another object, as `pytz` does for a different DST state.
fn same_tzinfo(tzinfo: Option<&Bound<'_, PyTzInfo>>, now: &Bound<'_, PyDateTime>) -> bool {
    match (tzinfo, now.get_tzinfo()) {
        (None, None) => true,
        (Some(tzinfo), Some(now)) => tzinfo.is(&now),
        _ => false,
    }
}

/// The units between `d` and `now`, which share a time zone. `None` when
/// none is above zero.
fn partials(d: WallClock, now: WallClock) -> Option<Partials> {
    // `delta.days * 24 * 60 * 60 + delta.seconds` floors to whole seconds.
    if (now.microseconds() - d.microseconds()).div_euclid(1_000_000) <= 0 {
        return None;
    }
    let mut total_months =
        (i64::from(now.year) - i64::from(d.year)) * 12 + i64::from(now.month) - i64::from(d.month);
    if d.day > now.day || (d.day == now.day && d.time() > now.time()) {
        total_months -= 1;
    }
    let (years, months) = (total_months.div_euclid(12), total_months.rem_euclid(12));
    let pivot = if years != 0 || months != 0 {
        let mut year = i64::from(d.year) + years;
        let mut month = i64::from(d.month) + months;
        if month > 12 {
            month -= 12;
            year += 1;
        }
        let month = u8::try_from(month).expect("month is between 1 and 12");
        WallClock {
            year: i32::try_from(year).expect("pivot is not after now"),
            month,
            day: MONTHS_DAYS[usize::from(month - 1)].min(d.day),
            microsecond: 0,
            ..d
        }
    } else {
        d
    };
    let mut remaining = now.microseconds() - pivot.microseconds();
    let mut partials = [years, months, 0, 0, 0, 0];
    for (partial, chunk) in partials[2..].iter_mut().zip(CHUNKS) {
        *partial = remaining.div_euclid(chunk);
        remaining -= chunk * *partial;
    }
    partials.iter().any(|&count| count != 0).then_some(partials)
}
