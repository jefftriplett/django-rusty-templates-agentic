"""Printing dates, times and datetimes matches Django's render_value_in_context."""

import datetime
import zoneinfo

import pytest
from django.template import Context, Engine as DjangoEngine
from django.test import override_settings
from django.utils import translation

from django_rusty_templates import Engine

LIBRARIES = {
    "tz": "django.templatetags.tz",
    "l10n": "django.templatetags.l10n",
    "i18n": "django.templatetags.i18n",
}
CHICAGO = zoneinfo.ZoneInfo("America/Chicago")


class OtherTzinfo(datetime.tzinfo):
    def utcoffset(self, value):
        return datetime.timedelta(hours=5, minutes=30)

    def dst(self, value):
        return datetime.timedelta(0)

    def tzname(self, value):
        return "OTHER"


class DateTimeSubclass(datetime.datetime):
    pass


VALUES = {
    "aware": datetime.datetime(2026, 3, 8, 7, 30, 15, tzinfo=datetime.timezone.utc),
    "zoned": datetime.datetime(2026, 11, 1, 1, 30, tzinfo=CHICAGO),
    "folded": datetime.datetime(2026, 11, 1, 1, 30, fold=1, tzinfo=CHICAGO),
    "naive": datetime.datetime(2026, 7, 4, 0, 0),
    "noon": datetime.datetime(2026, 7, 4, 12, 0),
    "other": datetime.datetime(2026, 1, 2, 3, 4, tzinfo=OtherTzinfo()),
    "subclass": DateTimeSubclass(2026, 1, 2, 3, 4, tzinfo=datetime.timezone.utc),
    "date": datetime.date(2026, 2, 28),
    "time": datetime.time(23, 59, 1),
    "aware_time": datetime.time(9, 15, tzinfo=datetime.timezone.utc),
}
SOURCES = ["{{ %s }}" % name for name in VALUES] + [
    "{% load tz %}{{ aware|localtime }}|{{ aware|utc }}|{{ aware|timezone:'Asia/Tokyo' }}",
    "{% load tz %}{{ naive|timezone:'Europe/Paris' }}|{{ aware|localtime|date:'H:i e' }}",
    "{% load tz %}{% localtime off %}{{ aware }}{{ aware|localtime }}{% endlocaltime %}",
    "{% load tz %}{% timezone 'Asia/Kolkata' %}{{ aware }}{% endtimezone %}{{ aware }}",
    "{% load l10n %}{% localize off %}{{ aware }}|{{ date }}{% endlocalize %}{{ date }}",
    "{% load i18n %}{% language 'de' %}{{ aware }}|{{ date }}{% endlanguage %}{{ date }}",
    "{% autoescape off %}{{ aware }}{% endautoescape %}",
    "{% for value in list %}{{ value }};{% endfor %}",
    "{% with value=aware %}{{ value }}{% endwith %}",
    '{{ aware|default:"" }}|{% firstof missing aware %}',
]


def render_both(source, values):
    django = DjangoEngine(libraries=LIBRARIES).from_string(source)
    rusty = Engine(libraries=LIBRARIES).from_string(source)
    return django.render(Context(values)), rusty.render(values)


@pytest.mark.parametrize("source", SOURCES)
@pytest.mark.parametrize("use_tz", [True, False])
@pytest.mark.parametrize("language", ["en", "de", "ja", "ar"])
def test_datetime_output_matches_django(source, use_tz, language):
    values = {**VALUES, "list": list(VALUES.values())}
    with (
        override_settings(USE_TZ=use_tz, TIME_ZONE="America/Chicago"),
        translation.override(language),
    ):
        django, rusty = render_both(source, values)
    assert rusty == django


@pytest.mark.parametrize(
    "formats",
    [
        {"DATETIME_FORMAT": r"\Y\e\s Y-m-d H:i:s.u O T"},
        {"DATE_FORMAT": "l jS F Y", "TIME_FORMAT": "g:i A"},
        {"DATETIME_FORMAT": "r", "DATE_FORMAT": "U"},
        {"DATETIME_FORMAT": "<b>&</b> N j, Y"},
    ],
)
def test_format_settings(formats):
    values = {**VALUES, "list": list(VALUES.values())}
    source = "{% for value in list %}{{ value }};{% endfor %}"
    with override_settings(USE_I18N=False, USE_TZ=True, **formats):
        django, rusty = render_both(source, values)
    assert rusty == django
