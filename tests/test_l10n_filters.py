"""The localize and unlocalize filters match django.templatetags.l10n."""

import datetime
import sys
import zoneinfo
from decimal import Decimal
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.test import override_settings
from django.utils import translation
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine


class Text(str):
    pass


class Number(int):
    pass


replacement = Library()


@replacement.filter(name="localize")
def not_localize(value):
    return "not localize"


module = ModuleType("l10n_filters_replacement")
module.register = replacement
sys.modules[module.__name__] = module
LIBRARIES = {
    "l10n": "django.templatetags.l10n",
    "tz": "django.templatetags.tz",
    "fake_l10n": module.__name__,
}
CHICAGO = zoneinfo.ZoneInfo("America/Chicago")

VALUES = {
    "int": 1234567,
    "negative": -98765,
    "big": 12345678901234567890123,
    "float": 1234567.891,
    "small_float": 0.5,
    "exp_float": 1.5e20,
    "tiny_float": 1e-7,
    "decimal": Decimal("1234.5678"),
    "true": True,
    "false": False,
    "text": "<b>1234</b>",
    "safe": mark_safe("<b>1234</b>"),
    "subclass_text": Text("<i>x</i>"),
    "subclass_int": Number(1234),
    "none": None,
    "list": [1, 2],
    "aware": datetime.datetime(2026, 3, 8, 7, 30, 15, tzinfo=datetime.UTC),
    "zoned": datetime.datetime(2026, 11, 1, 1, 30, tzinfo=CHICAGO),
    "naive": datetime.datetime(2026, 7, 4, 0, 0),
    "date": datetime.date(2026, 2, 28),
    "time": datetime.time(23, 59, 1),
}


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
    {},
    {"USE_THOUSAND_SEPARATOR": True},
    {"USE_THOUSAND_SEPARATOR": True, "NUMBER_GROUPING": (3, 2, 0)},
    {
        "USE_THOUSAND_SEPARATOR": True,
        "DECIMAL_SEPARATOR": "#",
        "THOUSAND_SEPARATOR": "_",
        "DATE_FORMAT": "Y/m/d",
        "DATETIME_FORMAT": "Y/m/d H:i",
        "TIME_FORMAT": "H.i",
        "USE_TZ": True,
        "TIME_ZONE": "Asia/Tokyo",
    },
]


@pytest.mark.parametrize("settings", range(len(SETTINGS)))
@pytest.mark.parametrize("language", ["en", "de", "fr", "hi"])
@pytest.mark.parametrize(
    "name", list(VALUES) + ["missing", "1234.5", "1234", '"<b>&</b>"']
)
@pytest.mark.parametrize("filters", ["localize", "unlocalize", "localize|length"])
def test_l10n_filters(settings, language, name, filters):
    source = "{% load l10n %}{{ " + name + "|" + filters + " }}"
    with override_settings(**SETTINGS[settings]), translation.override(language):
        django, rusty = render_both(source)
    assert rusty == django


@pytest.mark.parametrize(
    "source",
    [
        "{% localize off %}{{ float|localize }}|{{ date|localize }}{% endlocalize %}",
        "{% localize on %}{{ float|unlocalize }}|{{ date|unlocalize }}{% endlocalize %}",
        "{% autoescape off %}{{ text|localize }}{{ safe|unlocalize }}{% endautoescape %}",
        "{% for value in values %}{{ value|localize }};{{ value|unlocalize }};{% endfor %}",
        "{{ aware|localtime|localize }}|{{ aware|timezone:'Europe/Paris'|localize }}",
        "{% timezone 'Asia/Kolkata' %}{{ aware|localize }}{% endtimezone %}",
        "{{ int|localize|add:1 }}|{{ float|localize|floatformat:1 }}",
        "{% if int|localize == '1,234,567' %}yes{% else %}no{% endif %}",
    ],
)
@pytest.mark.parametrize("language", ["en", "de"])
def test_l10n_filters_with_tags(source, language):
    source = "{% load l10n tz %}" + source
    VALUES["values"] = list(VALUES.values())
    try:
        with (
            override_settings(USE_TZ=True, USE_THOUSAND_SEPARATOR=True),
            translation.override(language),
        ):
            django, rusty = render_both(source)
    finally:
        del VALUES["values"]
    assert rusty == django


def test_replacement_library_filters_stay_python():
    source = (
        "{% load l10n %}{% load fake_l10n %}{{ int|localize }}|{{ int|unlocalize }}"
    )
    django, rusty = render_both(source)
    assert rusty == django
    assert rusty.startswith("not localize|")
