import datetime
import zoneinfo

import pytest
from django.template import Context, Engine as DjangoEngine, engines
from django.template.defaultfilters import register
from django.utils import translation

from django_rusty_templates import Engine


@pytest.mark.parametrize("name", ["timesince", "timeuntil"])
@pytest.mark.parametrize(
    "value",
    [
        None,
        "",
        "invalid",
        3,
        datetime.date(2024, 1, 1),
        datetime.datetime(2024, 1, 1),
        datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone.utc),
    ],
)
@pytest.mark.parametrize("language", ["en", "de"])
def test_date_comparison(name, value, language):
    context = {"value": value, "reference": datetime.datetime(2024, 1, 3, 2, 30)}
    source = "{{ value|" + name + ":reference }}"
    with translation.override(language):
        if value in ("invalid", 3):
            for engine in (DjangoEngine(), Engine()):
                with pytest.raises(AttributeError):
                    engine.from_string(source).render(Context(context))
        else:
            expected = DjangoEngine().from_string(source).render(Context(context))
            assert Engine().from_string(source).render(Context(context)) == expected


@pytest.mark.parametrize("name", ["timesince", "timeuntil"])
def test_missing_date(name):
    source = "{{ missing|" + name + " }}"
    assert Engine().from_string(source).render({}) == ""


def render_both(source, context):
    """Render with Django and Rusty, returning output or the exception raised."""
    results = []
    for name in ("django", "rusty"):
        try:
            results.append(engines[name].from_string(source).render(context))
        except Exception as error:
            results.append((type(error), str(error)))
    return results


class OffsetTzinfo(datetime.tzinfo):
    """A tzinfo the native port leaves to Django."""

    def utcoffset(self, dt):
        return datetime.timedelta(hours=2)

    def dst(self, dt):
        return datetime.timedelta(0)


class DateTimeSubclass(datetime.datetime):
    pass


UTC = datetime.timezone.utc
CHICAGO = zoneinfo.ZoneInfo("America/Chicago")
WARSAW = zoneinfo.ZoneInfo("Europe/Warsaw")
MINUS_FIVE = datetime.timezone(datetime.timedelta(hours=-5))
DT = datetime.datetime


@pytest.mark.parametrize(
    "value,reference",
    [
        pytest.param(DT(2023, 1, 1), DT(2025, 3, 5, 4, 7), id="years_months"),
        pytest.param(DT(2024, 1, 1), DT(2024, 1, 22, 5), id="weeks_skip_days"),
        pytest.param(DT(2024, 1, 1), DT(2024, 1, 1, 5, 59, 59), id="hours_minutes"),
        pytest.param(DT(2024, 1, 1), DT(2024, 1, 1, 0, 0, 59), id="under_a_minute"),
        pytest.param(DT(2024, 1, 1), DT(2024, 1, 1), id="equal"),
        pytest.param(DT(2030, 1, 1), DT(2024, 1, 1), id="reversed"),
        pytest.param(DT(2024, 2, 29, 10), DT(2025, 2, 28, 10), id="leap_day"),
        pytest.param(DT(2024, 2, 29), DT(2028, 2, 29), id="leap_to_leap"),
        pytest.param(DT(2024, 1, 31), DT(2024, 3, 1), id="month_end_leap"),
        pytest.param(DT(2023, 1, 31), DT(2023, 3, 1), id="month_end"),
        pytest.param(DT(2024, 8, 31, 12), DT(2024, 9, 30, 11), id="short_month"),
        pytest.param(
            DT(2024, 1, 1, 10, 0, 0, 500000),
            DT(2024, 2, 1, 10, 0, 0, 300000),
            id="microseconds_before",
        ),
        pytest.param(
            DT(2024, 1, 1, 10, 0, 0, 300000),
            DT(2024, 2, 1, 10, 0, 0, 500000),
            id="microseconds_after",
        ),
        pytest.param(datetime.date(2023, 2, 28), DT(2024, 2, 29, 23), id="date"),
        pytest.param(
            datetime.date(2024, 1, 1), datetime.date(2024, 1, 15), id="two_dates"
        ),
        pytest.param(
            DT(2024, 3, 31, 1, tzinfo=WARSAW),
            DT(2024, 3, 31, 4, tzinfo=WARSAW),
            id="dst",
        ),
        pytest.param(
            DT(2024, 11, 2, 12, tzinfo=CHICAGO),
            DT(2024, 11, 3, 6, tzinfo=UTC),
            id="convert_to_value_zone",
        ),
        pytest.param(
            DT(2024, 1, 1, tzinfo=MINUS_FIVE),
            DT(2024, 1, 1, 3, tzinfo=UTC),
            id="offset",
        ),
        pytest.param(DT(2024, 1, 1), DT(2024, 1, 3, tzinfo=UTC), id="naive_aware"),
        pytest.param(
            datetime.date(2024, 1, 1), DT(2024, 1, 3, tzinfo=UTC), id="date_aware"
        ),
        pytest.param(
            DT(2024, 1, 1, tzinfo=OffsetTzinfo()),
            DT(2024, 1, 3, tzinfo=UTC),
            id="custom_tzinfo",
        ),
        pytest.param(
            DateTimeSubclass(2024, 1, 1), DT(2024, 2, 3), id="datetime_subclass"
        ),
        pytest.param(DT(2024, 1, 1), "invalid", id="invalid_reference"),
        pytest.param(DT(2024, 1, 1), "", id="empty_reference"),
        pytest.param(DT(1, 1, 1), DT(9999, 12, 31, 23, 59), id="extremes"),
    ],
)
@pytest.mark.parametrize("language", ["en", "pl", "ar"])
def test_matches_django(value, reference, language):
    source = (
        "[{{ value|timesince:reference }}][{{ value|timeuntil:reference }}]"
        "[{{ reference|timesince:value }}][{{ reference|timeuntil:value }}]"
        "{% autoescape off %}[{{ value|timesince:reference }}]{% endautoescape %}"
    )
    context = {"value": value, "reference": reference}
    with translation.override(language):
        django, rusty = render_both(source, context)
    assert rusty == django


@pytest.mark.parametrize("tzinfo", [None, UTC, CHICAGO])
def test_current_time(tzinfo):
    now = datetime.datetime.now(tzinfo)
    context = {
        "past": now - datetime.timedelta(days=10, hours=5),
        "future": now + datetime.timedelta(days=10, hours=5, minutes=30),
        "none": None,
    }
    source = (
        "{{ past|timesince }}|{{ future|timeuntil }}|{{ future|timesince }}"
        "|{{ past|timeuntil:none }}"
    )
    rusty = engines["rusty"].from_string(source).render(context)
    assert (
        rusty == "1\xa0week, 3\xa0days|1\xa0week, 3\xa0days|0\xa0minutes|0\xa0minutes"
    )


def test_follows_language_changes():
    def activate():
        translation.activate("pl")
        return ""

    source = (
        "{% load i18n %}{{ value|timesince:reference }}|"
        "{% language 'ar' %}{{ value|timesince:reference }}{% endlanguage %}|"
        "{{ value|timesince:reference }}{{ activate }}|{{ value|timesince:reference }}"
    )
    context = {
        "value": DT(2024, 1, 1),
        "reference": DT(2026, 2, 1),
        "activate": activate,
    }
    with translation.override("en"):
        django = engines["django"].from_string(source).render(context)
    with translation.override("en"):
        rusty = engines["rusty"].from_string(source).render(context)
    assert (
        rusty
        == django
        == (
            "2\xa0years, 1\xa0month|2\xa0سنتين، 1\xa0شهر|2\xa0years, 1\xa0month|"
            "2\xa0lata, 1\xa0miesiąc"
        )
    )


@pytest.mark.parametrize("name", ["timesince", "timeuntil"])
def test_replaced_builtin(monkeypatch, name):
    monkeypatch.setitem(register.filters, name, lambda value, arg=None: "replacement")
    django, rusty = render_both(
        "{{ value|" + name + " }}", {"value": datetime.date(2024, 1, 1)}
    )
    assert rusty == django == "replacement"
