"""Printing a Decimal matches Django's render_value_in_context."""

from decimal import Decimal

import pytest
from django.template import Context, Engine as DjangoEngine
from django.test import override_settings
from django.utils import translation

from django_rusty_templates import Engine


class Money(Decimal):
    pass


DECIMALS = [
    Decimal("0"),
    Decimal("-0"),
    Decimal("0.00"),
    Decimal("1234.5678"),
    Decimal("-1234567.80"),
    Decimal("1000000"),
    Decimal("12345678901234567890.123456789"),
    Decimal("0.000001"),
    Decimal("0.0000001"),
    Decimal("1E+3"),
    Decimal("1.5E-10"),
    Decimal("-2E+30"),
    Decimal("NaN"),
    Decimal("-Infinity"),
    Decimal("Infinity"),
    Decimal("9" * 99),
    Decimal("9" * 120),
    Decimal("1." + "1" * 150),
    Decimal("9" * 60 + "." + "1" * 150),
    Money("1234.5"),
]
SETTINGS = [
    {},
    {"USE_THOUSAND_SEPARATOR": True},
    {"USE_THOUSAND_SEPARATOR": True, "NUMBER_GROUPING": (3, 2, 0)},
    {
        "USE_THOUSAND_SEPARATOR": True,
        "DECIMAL_SEPARATOR": "#",
        "THOUSAND_SEPARATOR": "_",
    },
]
SOURCES = [
    "{% for value in values %}{{ value }};{% endfor %}",
    "{% load l10n %}{% localize off %}{% for value in values %}{{ value }};"
    "{% endfor %}{% endlocalize %}",
    "{% load l10n %}{% localize on %}{% for value in values %}{{ value }};"
    "{% endfor %}{% endlocalize %}",
    "{% load l10n %}{% for value in values %}{{ value|localize }};"
    "{{ value|unlocalize }};{% endfor %}",
    "{% autoescape off %}{% for value in values %}{{ value }};{% endfor %}"
    "{% endautoescape %}",
]


def outcome(render):
    try:
        return render()
    except Exception as error:  # noqa: BLE001
        # Django 5.2 raises TypeError for NaN; Rusty keeps that fallback.
        return type(error)


@pytest.mark.parametrize("source", SOURCES)
@pytest.mark.parametrize("settings", range(len(SETTINGS)))
@pytest.mark.parametrize("language", ["en", "de", "fr", "hi", "ar"])
def test_decimal_output_matches_django(source, settings, language):
    libraries = {"l10n": "django.templatetags.l10n"}
    with override_settings(**SETTINGS[settings]), translation.override(language):
        django = DjangoEngine(libraries=libraries).from_string(source)
        rusty = Engine(libraries=libraries).from_string(source)
        for value in DECIMALS:
            context = {"values": [value]}
            expected = outcome(lambda: django.render(Context(context)))
            assert outcome(lambda: rusty.render(context)) == expected, value
