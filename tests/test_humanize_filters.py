"""The humanize filters match django.contrib.humanize.

Rusty renders `intcomma`, `intword`, `ordinal`, `apnumber`, `naturalday` and
`naturaltime` natively for plain values, and calls Django's filter for
anything else.
"""

import datetime
import sys
import zoneinfo
from decimal import Decimal
from types import ModuleType
from unittest import mock

import pytest
import time_machine
from django.contrib.humanize.templatetags import humanize
from django.template import Context, Engine as DjangoEngine, Library
from django.test import override_settings
from django.utils import translation
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine

NOW = datetime.datetime(2026, 11, 7, 12, 0, 0, 250000, tzinfo=datetime.UTC)
NAIVE_NOW = NOW.replace(tzinfo=None)
CHICAGO = zoneinfo.ZoneInfo("America/Chicago")


class Text(str):
    pass


class Number(int):
    pass


replacement = Library()


@replacement.filter(name="intcomma")
def not_intcomma(value):
    return "not intcomma"


module = ModuleType("humanize_filters_replacement")
module.register = replacement
sys.modules[module.__name__] = module
LIBRARIES = {
    "humanize": "django.contrib.humanize.templatetags.humanize",
    "i18n": "django.templatetags.i18n",
    "fake_humanize": module.__name__,
}

NUMBERS = {
    "zero": 0,
    "one": 1,
    "two": 2,
    "three": 3,
    "nine": 9,
    "ten": 10,
    "eleven": 11,
    "twelve": 12,
    "thirteen": 13,
    "twenty_one": 21,
    "one_oh_one": 101,
    "one_eleven": 111,
    "negative": -1234567,
    "int": 1234567,
    "big": 12345678901234567890123,
    "huge": 10**250,
    "float": 1234567.891,
    "negative_float": -0.5,
    "negative_zero": -0.0,
    "exp_float": 1.5e20,
    "tiny_float": 1.5e-7,
    "inf": float("inf"),
    "nan": float("nan"),
    "decimal": Decimal("1234567.8900"),
    "decimal_exp": Decimal("1.2E+7"),
    "decimal_zero_exp": Decimal("0E+3"),
    "decimal_negative_zero": Decimal("-0.000"),
    "decimal_nan": Decimal("NaN"),
    "true": True,
    "false": False,
    "none": None,
    "text_int": "1234567",
    "text_float": "1234567.25",
    "text_padded": " 1234 ",
    "text_underscore": "1_234",
    "text_plus": "+1234",
    "text_exp": "1e5",
    "text_leading_zeros": "0012345",
    "text_negative_zero": "-0",
    "text_nan": "NaN",
    "text_word": "<b>abc</b>",
    "text_empty": "",
    "safe_int": mark_safe("1234567"),
    "safe_three": mark_safe("3"),
    "safe_big": mark_safe("12"),
    "safe_word": mark_safe("<b>abc</b>"),
    "subclass_text": Text("1234"),
    "subclass_int": Number(1234),
    "list": [1, 2],
    "million": 1000000,
    "almost_million": 999999,
    "million_and_half": 1500000,
    "near_billion": 999999999,
    "rounds_to_billion": 999950000,
    "billion": 1200000000,
    "negative_million": -1250000,
    "trillion": 31415926535897,
    "quadrillion": 4503599627370495,
    "past_exact": 2**53 + 1,
    "text_million": "2500000",
}

TIMES = {
    "now": NOW,
    "half_second_ago": NOW - datetime.timedelta(seconds=0.5),
    "second_ago": NOW - datetime.timedelta(seconds=1),
    "seconds_ago": NOW - datetime.timedelta(seconds=30),
    "minute_ago": NOW - datetime.timedelta(seconds=61),
    "minutes_ago": NOW - datetime.timedelta(minutes=59, seconds=59),
    "hour_ago": NOW - datetime.timedelta(hours=1),
    "hours_ago": NOW - datetime.timedelta(hours=23, minutes=59),
    "day_ago": NOW - datetime.timedelta(days=1, hours=2),
    "months_ago": NOW - datetime.timedelta(days=70),
    "week_ago": NOW - datetime.timedelta(days=7, seconds=5),
    "month_and_week_ago": NOW - datetime.timedelta(days=38),
    "year_ago": NOW - datetime.timedelta(days=365),
    "years_ago": NOW - datetime.timedelta(days=900, hours=5),
    "leap_day_ago": datetime.datetime(2024, 2, 29, 12, tzinfo=datetime.UTC),
    "day_ahead": NOW + datetime.timedelta(days=1, minutes=1),
    "weeks_ahead": NOW + datetime.timedelta(days=17),
    "years_ahead": NOW + datetime.timedelta(days=800),
    "naive_days_ago": NAIVE_NOW - datetime.timedelta(days=5, hours=3),
    "naive_months_ahead": NAIVE_NOW + datetime.timedelta(days=95),
    "second_ahead": NOW + datetime.timedelta(seconds=1),
    "minutes_ahead": NOW + datetime.timedelta(minutes=5, seconds=3),
    "hours_ahead": NOW + datetime.timedelta(hours=3),
    "days_ahead": NOW + datetime.timedelta(days=3),
    "naive": NAIVE_NOW - datetime.timedelta(minutes=12),
    "naive_ahead": NAIVE_NOW + datetime.timedelta(hours=2),
    "chicago": (NOW - datetime.timedelta(minutes=7)).astimezone(CHICAGO),
    "date": NOW.date(),
    "tomorrow_date": NOW.date() + datetime.timedelta(days=1),
    "yesterday_date": NOW.date() - datetime.timedelta(days=1),
    "late_tonight": NOW.replace(hour=23, minute=30),
    "early_today": NOW.replace(hour=0, minute=10),
    "tokyo": NOW.astimezone(zoneinfo.ZoneInfo("Asia/Tokyo")),
    "far_date": datetime.date(2020, 2, 29),
    "text": "2026-11-07",
}

VALUES = NUMBERS | TIMES


def outcome(render):
    try:
        return render()
    except Exception as error:  # noqa: BLE001
        # Rusty formats its own error messages, so compare the type only.
        return type(error)


def render_both(source, values=VALUES):
    django = DjangoEngine(libraries=LIBRARIES).from_string(source)
    rusty = Engine(libraries=LIBRARIES).from_string(source)
    return (
        outcome(lambda: django.render(Context(values))),
        outcome(lambda: rusty.render(values)),
    )


SETTINGS = [
    {},
    {"USE_THOUSAND_SEPARATOR": True},
    {"NUMBER_GROUPING": (3, 2, 0)},
    {"DECIMAL_SEPARATOR": "#", "THOUSAND_SEPARATOR": "'", "NUMBER_GROUPING": 0},
    {"USE_I18N": False},
]


@pytest.mark.parametrize("settings", range(len(SETTINGS)))
@pytest.mark.parametrize("language", ["en", "de", "fr", "hi"])
@pytest.mark.parametrize("name", list(NUMBERS) + ["missing", "1234", '"<b>&</b>"'])
@pytest.mark.parametrize(
    "filters",
    [
        "intcomma",
        "intword",
        "ordinal",
        "apnumber",
        "apnumber|add:1",
        "ordinal|length",
    ],
)
def test_number_filters(settings, language, name, filters):
    source = "{% load humanize %}{{ " + name + "|" + filters + " }}"
    with override_settings(**SETTINGS[settings]), translation.override(language):
        django, rusty = render_both(source)
    assert rusty == django


@pytest.mark.parametrize("use_tz", [True, False])
@pytest.mark.parametrize("language", ["en", "de", "ja"])
@pytest.mark.parametrize("name", list(TIMES) + ["missing", "int"])
def test_naturaltime(use_tz, language, name):
    source = "{% load humanize %}{{ " + name + "|naturaltime }}"
    with (
        time_machine.travel(NOW, tick=False),
        override_settings(USE_TZ=use_tz),
        translation.override(language),
    ):
        django, rusty = render_both(source)
    assert rusty == django


@pytest.mark.parametrize("use_tz", [True, False])
@pytest.mark.parametrize("language", ["en", "de", "ja"])
@pytest.mark.parametrize("name", list(TIMES) + ["missing", "int"])
@pytest.mark.parametrize("argument", ["", ":'Y-m-d'", ":'SHORT_DATE_FORMAT'", ":fmt"])
def test_naturalday(use_tz, language, name, argument):
    source = "{% load humanize %}{{ " + name + "|naturalday" + argument + " }}"
    values = VALUES | {"fmt": "j N Y"}
    with (
        time_machine.travel(NOW, tick=False),
        override_settings(USE_TZ=use_tz, TIME_ZONE="America/Chicago"),
        translation.override(language),
    ):
        django, rusty = render_both(source, values)
    assert rusty == django


@pytest.mark.parametrize(
    "source",
    [
        "{% autoescape off %}{{ text_word|intcomma }}{{ safe_word|apnumber }}"
        "{% endautoescape %}",
        "{% for value in values %}{{ value|intcomma }};{{ value|ordinal }};"
        "{{ value|apnumber }};{% endfor %}",
        "{% language 'de' %}{{ float|intcomma }}{{ three|apnumber }}{% endlanguage %}"
        "|{{ float|intcomma }}{{ three|apnumber }}",
        "{{ int|intcomma:False }}|{{ int|intcomma:True }}",
        "{% if int|intcomma == '1,234,567' %}yes{% else %}no{% endif %}",
        "{{ int|intcomma|ordinal }}|{{ two|ordinal|upper }}",
    ],
)
@pytest.mark.parametrize("language", ["en", "de"])
def test_humanize_filters_with_tags(source, language):
    source = "{% load humanize i18n %}" + source
    values = VALUES | {"values": list(NUMBERS.values())}
    with translation.override(language):
        django, rusty = render_both(source, values)
    assert rusty == django


def test_replacement_library_filters_stay_python():
    source = "{% load humanize %}{% load fake_humanize %}{{ int|intcomma }}"
    django, rusty = render_both(source)
    assert rusty == django == "not intcomma"


def test_patched_datetime_reaches_naturaltime():
    """Tests that patch humanize's `datetime` still see their clock."""

    class MockDateTime(datetime.datetime):
        @classmethod
        def now(cls, tz=None):
            return NOW + datetime.timedelta(hours=5)

    source = "{% load humanize %}{{ now|naturaltime }}"
    with mock.patch.object(humanize, "datetime", MockDateTime):
        django, rusty = render_both(source)
    assert rusty == django == "5\xa0hours ago"


@pytest.mark.parametrize(
    ("name", "expected"),
    [
        ("day_ago", "1\xa0day, 2\xa0hours ago"),
        ("month_and_week_ago", "1\xa0month, 1\xa0week ago"),
        ("years_ago", "2\xa0years, 5\xa0months ago"),
        ("weeks_ahead", "2\xa0weeks, 3\xa0days from now"),
        ("naive_days_ago", "5\xa0days, 3\xa0hours ago"),
    ],
)
def test_naturaltime_spans_of_a_day_or_more(name, expected):
    source = "{% load humanize %}{{ " + name + "|naturaltime }}"
    with time_machine.travel(NOW, tick=False), translation.override("en"):
        django, rusty = render_both(source)
    assert rusty == django == expected


@pytest.mark.parametrize("language", ["ru", "pl", "ar", "fr"])
def test_naturaltime_spans_use_the_languages_plural_forms(language):
    source = (
        "{% load humanize %}{{ day_ago|naturaltime }}|{{ years_ago|naturaltime }}"
        "|{{ weeks_ahead|naturaltime }}|{{ years_ahead|naturaltime }}"
    )
    with time_machine.travel(NOW, tick=False), translation.override(language):
        django, rusty = render_both(source)
    assert rusty == django
