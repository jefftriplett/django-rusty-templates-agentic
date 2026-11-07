"""Integers Rusty computes itself are localized as Django localizes ints.

`forloop.counter`, integer literals and filters such as `add` and `length`
produce native integers; Django prints them with `localize()`.
"""

import pytest
from django.template import Context, Engine as DjangoEngine
from django.test import override_settings
from django.utils import translation

from django_rusty_templates import Engine

VALUES = {
    "rows": range(1500),
    "number": 1234567,
    "text": "x" * 2000,
    "words": " ".join(["w"] * 1200),
    "negative": -98765,
}

SOURCES = [
    "{% for x in rows %}{% if forloop.last %}{{ forloop.counter }}|"
    "{{ forloop.counter0 }}|{{ forloop.revcounter }}|{{ forloop.length }}"
    "{% endif %}{% endfor %}",
    "{{ 12345 }}|{{ -12345 }}|{{ 123 }}|{{ 123456789012345678901234567890 }}",
    "{{ number|add:1 }}|{{ negative|add:-1000 }}|{{ text|length }}",
    "{{ words|wordcount }}|{{ number|get_digit:1 }}",
    "{% widthratio 1500 1 1 %}",
    "{% localize off %}{{ 12345 }}|{{ number|add:1 }}{% endlocalize %}",
    "{% autoescape off %}{{ 12345 }}{% endautoescape %}",
]

SETTINGS = [
    {},
    {"USE_THOUSAND_SEPARATOR": True},
    {"USE_THOUSAND_SEPARATOR": True, "NUMBER_GROUPING": (3, 2, 0)},
    {"USE_THOUSAND_SEPARATOR": True, "THOUSAND_SEPARATOR": "'"},
]


@pytest.mark.parametrize("settings", range(len(SETTINGS)))
@pytest.mark.parametrize("language", ["en", "de", "de-ch", "hi"])
@pytest.mark.parametrize("source", SOURCES)
def test_native_integers_are_localized(settings, language, source):
    source = "{% load l10n %}" + source
    with override_settings(**SETTINGS[settings]), translation.override(language):
        django = DjangoEngine(
            libraries={"l10n": "django.templatetags.l10n"}
        ).from_string(source)
        rusty = Engine(libraries={"l10n": "django.templatetags.l10n"}).from_string(
            source
        )
        assert rusty.render(VALUES) == django.render(Context(VALUES))
