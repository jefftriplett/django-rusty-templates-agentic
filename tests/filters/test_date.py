"""
Adapted Some tests from
https://github.com/django/django/blob/5.1/tests/template_tests/filter_tests/test_date.py
"""

from inline_snapshot import snapshot

from datetime import datetime, time, timezone as dt_timezone

import pytest
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
