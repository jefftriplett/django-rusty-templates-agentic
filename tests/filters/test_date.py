"""
Adapted Some tests from
https://github.com/django/django/blob/5.1/tests/template_tests/filter_tests/test_date.py
"""

from inline_snapshot import snapshot

import zoneinfo
from datetime import date, datetime, time, timedelta
from datetime import timezone as dt_timezone
from datetime import tzinfo as datetime_tzinfo

import pytest
from django.template import engines
from django.test import override_settings
from django.utils import timezone, translation


@pytest.mark.parametrize(
    "template,context,expected",
    [
        pytest.param(
            '{{ d|date:"m" }}',
            {"d": datetime(2008, 1, 1)},
            "01",
            id="test_date_01_month",
        ),
        pytest.param(
            "{{ d|date }}",
            {"d": datetime(2008, 1, 1)},
            "Jan. 1, 2008",
            id="test_date_02_default_format",
        ),
        pytest.param(
            "{{ d|date }}",
            {"d": "fail_string"},
            "",
            id="test_date_03_non_date",
        ),
        pytest.param(
            '{{ d|date:"o" }}',
            {"d": datetime(2008, 12, 29)},
            "2009",
            id="test_date_04_iso_year",
        ),
        pytest.param(
            '{{ d|date:"o" }}',
            {"d": datetime(2010, 1, 3)},
            "2009",
            id="test_date_05_iso_year_edge",
        ),
        pytest.param(
            '{{ d|date:"e" }}',
            {"d": datetime(2009, 3, 12, tzinfo=timezone.get_fixed_timezone(30))},
            "+0030",
            id="test_date_06_timezone_name",
        ),
        pytest.param(
            '{{ d|date:"e" }}',
            {"d": datetime(2009, 3, 12)},
            "",
            id="test_date_07_naive_datetime_timezone",
        ),
        pytest.param(
            '{{ t|date:"H:i" }}',
            {"t": time(0, 1)},
            "00:01",
            id="test_date_08_midnight_time",
        ),
        pytest.param(
            '{{ t|date:"H:i" }}',
            {"t": time(0, 0)},
            "00:00",
            id="test_date_09_exact_midnight",
        ),
    ],
)
def test_date(assert_render, template, context, expected):
    assert_render(template, context, expected)


@pytest.mark.parametrize(
    "template,context,expected",
    [
        pytest.param(
            "{{ var|date:'Y' }}",
            {"var": None},
            "",
            id="test_date_none",
        ),
        pytest.param(
            "{{ missing|date:'Y' }}",
            {},
            "",
            id="test_date_missing",
        ),
    ],
)
def test_date_empty_inputs(assert_render, template, context, expected):
    assert_render(template, context, expected)


@pytest.mark.parametrize(
    "template,context,expected",
    [
        pytest.param(
            '{{ d|date:"d F Y" }}',
            {"d": datetime(2005, 12, 29)},
            "29 December 2005",
            id="long_format",
        ),
        pytest.param(
            r'{{ d|date:"jS \o\f F" }}',
            {"d": datetime(2005, 12, 29)},
            "29th of December",
            id="escaped_characters",
        ),
        pytest.param('{{ d|date:"d F Y" }}', {"d": ""}, "", id="empty_string_input"),
        pytest.param('{{ d|date:"d F Y" }}', {"d": None}, "", id="none_input"),
    ],
)
def test_date_functional_scenarios(assert_render, template, context, expected):
    assert_render(template, context, expected)


@pytest.mark.parametrize(
    "template,context,expected",
    [
        pytest.param("{{ d|date:'Y' }}", {"d": 123}, "", id="test_date_int_input"),
        pytest.param("{{ d|date:'Y' }}", {"d": 1.2}, "", id="test_date_float_input"),
        pytest.param("{{ 123|date:'Y' }}", {}, "", id="test_date_int_literal"),
        pytest.param("{{ 1.2|date:'Y' }}", {}, "", id="test_date_float_literal"),
        pytest.param(
            "{{ d|date:'Y' }}", {"d": [2024, 1, 1]}, "", id="test_date_list_input"
        ),
        pytest.param(
            "{{ t|date:'Y' }}", {"t": time(12, 0)}, "", id="test_time_obj_with_year_fmt"
        ),
        pytest.param(
            "{{ t|date:'H' }}",
            {"t": time(13, 30)},
            "13",
            id="test_time_obj_with_hour_fmt",
        ),
    ],
)
def test_date_edges(assert_render, template, context, expected):
    assert_render(template, context, expected)


def test_date_localized(assert_render):
    template = "{{ d|date }}"
    with translation.override("fr"):
        assert_render(
            template=template,
            context={"d": datetime(2008, 1, 1)},
            expected="1 janvier 2008",
        )


def test_date_lazy_format(assert_render):
    template = '{{ t|date:_("H:i") }}'
    assert_render(
        template=template,
        context={"t": time(0, 0)},
        expected="00:00",
    )


def test_date_unexpected_argument(assert_parse_error):
    template = '{{ value|date:"Y":"m" }}'
    django_message = snapshot(
        'Could not parse the remainder: \':"m"\' from \'value|date:"Y":"m"\''
    )
    rusty_message = snapshot("""\
  × Expected a valid filter name
   ╭────
 1 │ {{ value|date:"Y":"m" }}
   ·                   ─┬─
   ·                    ╰── here
   ╰────
""")
    assert_parse_error(
        template=template,
        django_message=django_message,
        rusty_message=rusty_message,
    )


def test_date_error_propagation(assert_render_error):
    from datetime import date

    class ExplodingDate(date):
        @property
        def year(self):
            raise RuntimeError("Python execution failed")

    assert_render_error(
        template='{{ d|date:"Y" }}',
        context={"d": ExplodingDate(2024, 1, 1)},
        exception=RuntimeError,
        django_message=snapshot("Python execution failed"),
        rusty_message=snapshot("""\
  × Python execution failed
   ╭────
 1 │ {{ d|date:"Y" }}
   ·      ──┬─
   ·        ╰── here
   ╰────
"""),
    )


AWARE_UTC = datetime(2026, 10, 17, 3, 30, tzinfo=dt_timezone.utc)
AWARE_PLUS_0530 = datetime(2026, 10, 17, 3, 30, tzinfo=timezone.get_fixed_timezone(330))
NAIVE = datetime(2026, 10, 17, 3, 30)
CHICAGO = {"USE_TZ": True, "TIME_ZONE": "America/Chicago"}
KOLKATA = {"USE_TZ": True, "TIME_ZONE": "Asia/Kolkata"}
NO_TZ = {"USE_TZ": False, "TIME_ZONE": "America/Chicago"}


@pytest.mark.parametrize(
    "tz_settings,template,value,expected",
    [
        pytest.param(
            CHICAGO,
            '{{ d|date:"Y-m-d H:i T" }}',
            AWARE_UTC,
            "2026-10-16 22:30 CDT",
            id="aware_utc_to_chicago",
        ),
        pytest.param(
            CHICAGO,
            '{{ d|date:"Y-m-d H:i O" }}',
            AWARE_PLUS_0530,
            "2026-10-16 17:00 -0500",
            id="aware_offset_to_chicago",
        ),
        pytest.param(
            KOLKATA,
            '{{ d|date:"Y-m-d H:i" }}',
            AWARE_UTC,
            "2026-10-17 09:00",
            id="aware_utc_to_kolkata",
        ),
        pytest.param(
            CHICAGO,
            '{{ d|date:"Y-m-d H:i" }}',
            NAIVE,
            "2026-10-17 03:30",
            id="naive_unchanged",
        ),
        pytest.param(
            NO_TZ,
            '{{ d|date:"Y-m-d H:i e" }}',
            AWARE_UTC,
            "2026-10-17 03:30 UTC",
            id="use_tz_false_unchanged",
        ),
        pytest.param(
            CHICAGO,
            '{% load tz %}{% localtime off %}{{ d|date:"Y-m-d H:i" }}{% endlocaltime %}',
            AWARE_UTC,
            "2026-10-17 03:30",
            id="localtime_off_unchanged",
        ),
        pytest.param(
            CHICAGO,
            '{% load tz %}{% timezone "Asia/Kolkata" %}{{ d|date:"Y-m-d H:i" }}'
            "{% endtimezone %}",
            AWARE_UTC,
            "2026-10-17 09:00",
            id="timezone_tag",
        ),
        pytest.param(
            CHICAGO, "{{ d|date }}", AWARE_UTC, "Oct. 16, 2026", id="default_format"
        ),
        pytest.param(
            CHICAGO,
            '{{ d|date:"SHORT_DATE_FORMAT" }}',
            AWARE_UTC,
            "10/16/2026",
            id="short_date_format_aware",
        ),
        pytest.param(
            CHICAGO,
            '{{ d|date:"DATETIME_FORMAT" }}',
            AWARE_UTC,
            "Oct. 16, 2026, 10:30 p.m.",
            id="datetime_format_aware",
        ),
        pytest.param(
            CHICAGO,
            '{{ d|date:"SHORT_DATETIME_FORMAT" }}',
            NAIVE,
            "10/17/2026 3:30 a.m.",
            id="short_datetime_format_naive",
        ),
        pytest.param(
            NO_TZ,
            '{{ d|date:"SHORT_DATE_FORMAT" }}',
            NAIVE.date(),
            "10/17/2026",
            id="short_date_format_date",
        ),
    ],
)
def test_date_localtime_and_format_names(
    assert_render, tz_settings, template, value, expected
):
    with override_settings(**tz_settings):
        assert_render(template, {"d": value}, expected)


@pytest.mark.parametrize(
    "tz_settings,template,value,expected",
    [
        pytest.param(
            CHICAGO, '{{ d|time:"H:i" }}', AWARE_UTC, "22:30", id="aware_utc_to_chicago"
        ),
        pytest.param(
            KOLKATA, '{{ d|time:"H:i" }}', AWARE_UTC, "09:00", id="aware_utc_to_kolkata"
        ),
        pytest.param(
            CHICAGO, '{{ d|time:"H:i" }}', NAIVE, "03:30", id="naive_unchanged"
        ),
        pytest.param(
            CHICAGO, '{{ d|time:"H:i" }}', NAIVE.time(), "03:30", id="time_unchanged"
        ),
        pytest.param(
            NO_TZ, '{{ d|time:"H:i" }}', AWARE_UTC, "03:30", id="use_tz_false_unchanged"
        ),
        pytest.param(
            CHICAGO,
            '{% load tz %}{% localtime off %}{{ d|time:"H:i" }}{% endlocaltime %}',
            AWARE_UTC,
            "03:30",
            id="localtime_off_unchanged",
        ),
        pytest.param(
            CHICAGO,
            '{% load tz %}{% timezone "Asia/Kolkata" %}{{ d|time:"H:i" }}'
            "{% endtimezone %}",
            AWARE_UTC,
            "09:00",
            id="timezone_tag",
        ),
        pytest.param(
            CHICAGO, "{{ d|time }}", AWARE_UTC, "10:30 p.m.", id="default_format"
        ),
        pytest.param(
            CHICAGO,
            '{{ d|time:"TIME_FORMAT" }}',
            AWARE_UTC,
            "10:30 p.m.",
            id="time_format_aware",
        ),
    ],
)
def test_time_localtime_and_format_names(
    assert_render, tz_settings, template, value, expected
):
    with override_settings(**tz_settings):
        assert_render(template, {"d": value}, expected)


# Differential tests: the rusty engine must render exactly what Django renders.

FORMAT_CHARACTERS = "aAbcdDeEfFgGhHiIjlLmMnNoOPrsStTUuwWyYzZ"
# Specifiers that depend on the timezone and are formatted by Django.
FALLBACK_CHARACTERS = "eIOrTUZ"

UTC = dt_timezone.utc
TOKYO = zoneinfo.ZoneInfo("Asia/Tokyo")

DATETIMES = [
    # 22:30 on the previous day in America/Chicago.
    datetime(2026, 10, 17, 3, 30, tzinfo=UTC),
    datetime(2026, 5, 11, 1, 1, tzinfo=TOKYO),
    datetime(9999, 12, 31, 11, 1, tzinfo=dt_timezone(timedelta(hours=-3, minutes=-30))),
    datetime(2024, 2, 29, 0, 0),
    datetime(2024, 12, 30, 12, 0, 5, 7),
    datetime(2021, 1, 3, 23, 59, 59, 999999),
    datetime(2026, 3, 22, 12, 30),
    datetime(1, 1, 1, 13, 7),
]
DATES = [
    date(2008, 12, 29),
    date(2010, 1, 3),
    date(2023, 1, 1),
    date(2000, 2, 29),
    date(1900, 3, 1),
    date(2026, 11, 13),
    date(2026, 8, 21),
    date(2026, 7, 2),
    date(999, 6, 23),
]
TIMES = [time(0, 0), time(0, 1), time(12, 0), time(12, 30, 15, 250), time(23, 5)]


def render_with(engine, template, context):
    try:
        return engines[engine].from_string(template).render(context)
    except Exception as error:
        return type(error)


def assert_matches_django(template, context):
    expected = render_with("django", template, context)
    assert (template, render_with("rusty", template, context)) == (template, expected)


@pytest.fixture
def chicago(settings):
    settings.USE_TZ = True
    settings.TIME_ZONE = "America/Chicago"


def test_date_converts_aware_datetimes_to_the_current_timezone(chicago, assert_render):
    assert_render(
        '{{ d|date:"Y-m-d H:i" }} {{ d|date:"SHORT_DATE_FORMAT" }} {{ d|time:"H:i" }}',
        {"d": datetime(2026, 10, 17, 3, 30, tzinfo=UTC)},
        "2026-10-16 22:30 10/16/2026 22:30",
    )


@pytest.mark.parametrize("use_tz", [True, False])
@pytest.mark.parametrize("character", FORMAT_CHARACTERS)
def test_date_format_character_matches_django(settings, use_tz, character):
    settings.USE_TZ = use_tz
    settings.TIME_ZONE = "America/Chicago"
    for value in DATETIMES + DATES + TIMES:
        for name in ("date", "time"):
            template = f'{{{{ value|{name}:"{character}" }}}}'
            assert_matches_django(template, {"value": value})


@pytest.mark.parametrize(
    "format_string",
    [
        "D, d M Y H:i:s O",
        "N j, Y, P T",
        "c e",
        "r (U)",
        "I Z H:i",
        r"jS \o\f F",
        r"\\Y \\\Y",
        r"\Y\m\d",
        "Y\\",
        "H\\\ni",
        "é Y ü 日",
        "<b>Y</b> & m",
    ],
)
def test_date_mixed_and_escaped_formats_match_django(chicago, format_string):
    for value in DATETIMES + DATES + TIMES:
        for name in ("date", "time"):
            template = f"{{{{ value|{name}:format }}}}"
            assert_matches_django(template, {"value": value, "format": format_string})


@pytest.mark.parametrize(
    "value",
    [
        # Spring forward: 02:00 CST becomes 03:00 CDT at 08:00 UTC.
        datetime(2026, 3, 8, 7, 59, tzinfo=UTC),
        datetime(2026, 3, 8, 8, 0, tzinfo=UTC),
        # Fall back: 01:00-02:00 happens twice, at 06:00 and 07:00 UTC.
        datetime(2026, 11, 1, 6, 30, tzinfo=UTC),
        datetime(2026, 11, 1, 7, 30, tzinfo=UTC),
        datetime(2026, 11, 1, 1, 30, tzinfo=zoneinfo.ZoneInfo("America/Chicago")),
        datetime(2026, 11, 1, 1, 30, fold=1),
        datetime(2026, 11, 1, 1, 30),
        datetime(2026, 3, 8, 2, 30),
    ],
)
@pytest.mark.parametrize(
    "template",
    [
        '{{ value|date:"Y-m-d H:i P" }}|{{ value|time:"H:i" }}',
        '{{ value|date:"c I O T Z e" }}|{{ value|time:"O T e" }}',
        '{% load tz %}{% localtime off %}{{ value|date:"Y-m-d H:i O" }}|{{ value|time:"H:i" }}{% endlocaltime %}',
        '{% load tz %}{% timezone "Asia/Tokyo" %}{{ value|date:"Y-m-d H:i O" }}|{{ value|time:"H:i" }}{% endtimezone %}{{ value|date:"H:i" }}',
    ],
)
@pytest.mark.parametrize("use_tz", [True, False])
def test_date_timezones_and_dst_match_django(settings, use_tz, value, template):
    settings.USE_TZ = use_tz
    settings.TIME_ZONE = "America/Chicago"
    assert_matches_django(template, {"value": value})


@pytest.mark.parametrize("language", ["en", "de", "fr", "pl", "ru", "ja"])
@pytest.mark.parametrize(
    "argument",
    [
        "",
        ':""',
        ":format",
        ':"DATE_FORMAT"',
        ':"DATETIME_FORMAT"',
        ':"SHORT_DATE_FORMAT"',
        ':"SHORT_DATETIME_FORMAT"',
        ':"TIME_FORMAT"',
        ':"YEAR_MONTH_FORMAT"',
        ':"MONTH_DAY_FORMAT"',
        # Not strings: formatted by Django.
        ':"FIRST_DAY_OF_WEEK"',
        ':"DATE_INPUT_FORMATS"',
    ],
)
def test_date_format_settings_match_django(chicago, language, argument):
    context = {"format": None}
    with translation.override(language):
        for value in [DATETIMES[0], DATETIMES[3], DATES[0], TIMES[3]]:
            context["value"] = value
            for name in ("date", "time"):
                assert_matches_django(f"{{{{ value|{name}{argument} }}}}", context)


@pytest.mark.parametrize("language", ["en", "de", "fr", "pl", "ru", "ja"])
def test_date_names_match_django(language):
    context = {
        # Every month, and every weekday twice.
        "dates": [date(2026, month, month) for month in range(1, 13)]
        + [date(2026, 6, day) for day in range(1, 8)],
        "times": TIMES,
    }
    template = (
        '{% for d in dates %}{{ d|date:"l D F b M N E jS" }}|{% endfor %}'
        '{% for t in times %}{{ t|date:"a A P f" }}|{{ t|time:"a A P" }}|{% endfor %}'
    )
    with translation.override(language):
        assert_matches_django(template, context)


def test_date_language_tag_matches_django(chicago):
    template = (
        '{% load i18n %}{{ d|date:"l F P" }}|'
        '{% language "de" %}{{ d|date:"l F P" }}|'
        '{% language "fr" %}{{ d|date:"l F P DATE_FORMAT" }}|{% endlanguage %}'
        '{{ d|date:"l F P" }}|{% endlanguage %}{{ d|date:"l F P" }}'
    )
    assert_matches_django(template, {"d": DATETIMES[0]})


@pytest.mark.parametrize(
    "value",
    [
        "",
        "2026-10-17",
        0,
        None,
        datetime(2026, 10, 17, 3, 30, tzinfo=datetime_tzinfo()),
    ],
)
def test_date_unsupported_values_match_django(chicago, value):
    for name in ("date", "time"):
        assert_matches_django(f'{{{{ value|{name}:"Y-m-d H:i" }}}}', {"value": value})
        assert_matches_django(f'{{{{ value|{name}:"literal" }}}}', {"value": value})
