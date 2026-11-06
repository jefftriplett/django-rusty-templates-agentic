"""The localtime, utc and timezone filters match django.templatetags.tz."""

import datetime
import sys
import zoneinfo
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.test import override_settings

from django_rusty_templates import Engine


class OtherTzinfo(datetime.tzinfo):
    def utcoffset(self, value):
        return datetime.timedelta(hours=5, minutes=30)

    def dst(self, value):
        return datetime.timedelta(0)

    def tzname(self, value):
        return "OTHER"


class NoOffset(datetime.tzinfo):
    def utcoffset(self, value):
        return None

    def dst(self, value):
        return None

    def tzname(self, value):
        return None


class DateTimeSubclass(datetime.datetime):
    pass


replacement = Library()


@replacement.filter(name="utc")
def not_utc(value):
    return f"not utc: {value}"


@replacement.filter(name="localtime")
def not_localtime(value):
    return "not localtime"


module = ModuleType("tz_filters_replacement")
module.register = replacement
sys.modules[module.__name__] = module
LIBRARIES = {"tz": "django.templatetags.tz", "fake_tz": module.__name__}
CHICAGO = zoneinfo.ZoneInfo("America/Chicago")

VALUES = {
    "aware": datetime.datetime(2026, 3, 8, 7, 30, 15, 12, tzinfo=datetime.UTC),
    "zoned": datetime.datetime(2026, 11, 1, 1, 30, tzinfo=CHICAGO),
    "folded": datetime.datetime(2026, 11, 1, 1, 30, fold=1, tzinfo=CHICAGO),
    "naive": datetime.datetime(2026, 7, 4, 0, 0),
    "other": datetime.datetime(2026, 1, 2, 3, 4, tzinfo=OtherTzinfo()),
    "no_offset": datetime.datetime(2026, 1, 2, 3, 4, tzinfo=NoOffset()),
    "subclass": DateTimeSubclass(2026, 1, 2, 3, 4, tzinfo=datetime.UTC),
    "date": datetime.date(2026, 2, 28),
    "time": datetime.time(23, 59, 1),
    "text": "2026-01-02",
    "number": 7,
    "none": None,
    "tokyo": "Asia/Tokyo",
    "bad_zone": "Not/AZone",
    "tzinfo": OtherTzinfo(),
    "zoneinfo": zoneinfo.ZoneInfo("Europe/Paris"),
}
NAMES = [name for name in VALUES if name not in ("tokyo", "bad_zone", "tzinfo")]
ARGUMENTS = [
    "'Asia/Tokyo'",
    "'Europe/Paris'",
    "'UTC'",
    "'Not/AZone'",
    "''",
    "tokyo",
    "bad_zone",
    "tzinfo",
    "zoneinfo",
    "number",
    "none",
    "missing",
    "1",
]


def outcome(render):
    try:
        return render()
    except Exception as error:  # noqa: BLE001
        # Rusty formats its own error messages, so compare the type only.
        return type(error)


def render_both(source):
    django = DjangoEngine(libraries=LIBRARIES).from_string(source)
    rusty = Engine(libraries=LIBRARIES).from_string(source)
    return (
        outcome(lambda: django.render(Context(VALUES))),
        outcome(lambda: rusty.render(VALUES)),
    )


SETTINGS = [
    {"USE_TZ": True, "TIME_ZONE": "America/Chicago"},
    {"USE_TZ": False, "TIME_ZONE": "America/Chicago"},
    {"USE_TZ": True, "TIME_ZONE": "UTC"},
]


@pytest.mark.parametrize("settings", SETTINGS)
@pytest.mark.parametrize("name", NAMES + ["missing"])
@pytest.mark.parametrize(
    "filters",
    [
        "localtime",
        "utc",
        "localtime|date:'c e I T'",
        "utc|date:'c e'",
        "localtime|localtime",
        "utc|localtime",
        "localtime|default:'x'",
    ],
)
def test_localtime_and_utc(settings, name, filters):
    source = "{% load tz %}{{ " + name + "|" + filters + " }}"
    with override_settings(**settings):
        django, rusty = render_both(source)
    assert rusty == django


@pytest.mark.parametrize("settings", SETTINGS)
@pytest.mark.parametrize("name", NAMES + ["missing"])
@pytest.mark.parametrize("argument", ARGUMENTS)
def test_timezone(settings, name, argument):
    source = (
        "{% load tz %}{{ " + name + "|timezone:" + argument + " }}"
        "|{{ " + name + "|timezone:" + argument + "|date:'c e' }}"
    )
    with override_settings(**settings):
        django, rusty = render_both(source)
    assert rusty == django


@pytest.mark.parametrize(
    "source",
    [
        "{% timezone 'Asia/Kolkata' %}{{ aware|localtime }}{% endtimezone %}",
        "{% localtime off %}{{ aware|localtime }}|{{ aware|utc }}{% endlocaltime %}",
        "{% localtime off %}{{ aware|timezone:'Asia/Tokyo' }}{% endlocaltime %}",
        "{% for value in list %}{{ value|utc }};{{ value|localtime }};{% endfor %}",
        "{% if aware|utc %}yes{% endif %}",
        "{% with value=aware|timezone:'Asia/Tokyo' %}{{ value|date:'H' }}{% endwith %}",
        "{{ aware|utc|timezone:'Asia/Tokyo' }}",
    ],
)
@pytest.mark.parametrize("use_tz", [True, False])
def test_tz_filters_with_tags(source, use_tz):
    source = "{% load tz %}" + source
    VALUES["list"] = [VALUES[name] for name in NAMES]
    try:
        with override_settings(USE_TZ=use_tz, TIME_ZONE="America/Chicago"):
            django, rusty = render_both(source)
    finally:
        del VALUES["list"]
    assert rusty == django


def test_replacement_library_filters_stay_python():
    source = "{% load tz %}{% load fake_tz %}{{ aware|utc }}|{{ aware|localtime }}"
    with override_settings(USE_TZ=True):
        django, rusty = render_both(source)
    assert rusty == django
    assert rusty.startswith("not utc: ")
