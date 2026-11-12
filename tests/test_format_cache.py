"""Number formats are cached across renders but follow every change Django
follows (#368): `override_settings` of a format setting, the active language,
and `USE_THOUSAND_SEPARATOR` set directly."""

import pytest
from django.conf import settings
from django.template import Context, Engine as DjangoEngine
from django.test import override_settings
from django.utils import formats, translation

from django_rusty_templates import Engine

SOURCE = (
    "{% load l10n %}{{ number }}|{{ 1234567 }}|{{ price }}|{{ items|length }}"
    "{% for x in items %}{% if forloop.last %}|{{ forloop.counter }}{% endif %}"
    "{% endfor %}|{% localize on %}{{ number }}{% endlocalize %}"
)
CONTEXT = {"number": 9876543, "price": 1234.5, "items": range(1200)}
LIBRARIES = {"l10n": "django.templatetags.l10n"}


def pair():
    django = DjangoEngine(libraries=LIBRARIES).from_string(SOURCE)
    rusty = Engine(libraries=LIBRARIES).from_string(SOURCE)
    return django, rusty


def check(django, rusty):
    expected = django.render(Context(dict(CONTEXT)))
    assert rusty.render(dict(CONTEXT)) == expected
    return expected


STEPS = [
    {},
    {"USE_THOUSAND_SEPARATOR": True},
    {"USE_THOUSAND_SEPARATOR": True, "THOUSAND_SEPARATOR": "'"},
    {"USE_THOUSAND_SEPARATOR": True, "NUMBER_GROUPING": (3, 2, 0)},
    {"USE_THOUSAND_SEPARATOR": True, "DECIMAL_SEPARATOR": ";"},
    {"USE_THOUSAND_SEPARATOR": True, "NUMBER_GROUPING": 0},
]


@pytest.mark.parametrize("language", ["en", "de", "fr", "hi", "xx"])
def test_settings_changes_between_renders(language):
    """`xx` has no locale formats, so Django falls back to the settings and
    every step changes the output. The others use their locale's separators
    and only follow `USE_THOUSAND_SEPARATOR`."""
    django, rusty = pair()
    seen = set()
    with translation.override(language):
        for step in STEPS + STEPS[::-1]:
            with override_settings(**step):
                seen.add(check(django, rusty))
            seen.add(check(django, rusty))
    assert len(seen) > (2 if language == "xx" else 1)


def test_language_changes_between_renders():
    django, rusty = pair()
    with override_settings(USE_THOUSAND_SEPARATOR=True):
        outputs = []
        for language in ["en", "de", "en", "de-ch", "fr", "en", "hi", "de"] * 3:
            with translation.override(language):
                outputs.append(check(django, rusty))
    assert len(set(outputs)) > 2


def test_language_tag_inside_a_render():
    source = (
        "{% load i18n %}{{ number }}{% language 'de' %}|{{ number }}{% endlanguage %}"
        "{% language 'fr' %}|{{ number }}{% endlanguage %}|{{ number }}"
    )
    django = DjangoEngine(libraries={"i18n": "django.templatetags.i18n"}).from_string(
        source
    )
    rusty = Engine(libraries={"i18n": "django.templatetags.i18n"}).from_string(source)
    with override_settings(USE_THOUSAND_SEPARATOR=True):
        for _ in range(3):
            expected = django.render(Context({"number": 1234567}))
            assert rusty.render({"number": 1234567}) == expected
    assert "." in expected and "," in expected


def test_thousand_separator_set_directly():
    """Django reads `USE_THOUSAND_SEPARATOR` on every number, with no signal."""
    django, rusty = pair()
    previous = settings.USE_THOUSAND_SEPARATOR
    try:
        for value in [False, True, False, True]:
            settings.USE_THOUSAND_SEPARATOR = value
            check(django, rusty)
    finally:
        settings.USE_THOUSAND_SEPARATOR = previous


def test_reset_format_cache():
    django, rusty = pair()
    with override_settings(USE_THOUSAND_SEPARATOR=True):
        before = check(django, rusty)
        formats.reset_format_cache()
        assert check(django, rusty) == before


@pytest.mark.parametrize(
    "source",
    [
        "{% with a=1 %}{% language 'de' %}{% with b=1 %}{{ n }}{% endwith %}"
        "{% endlanguage %}{{ n }}{% endwith %}{{ n }}",
        "{% with a=1 %}{{ n }}{% language 'de' %}{{ n }}{% with b=1 %}{{ n }}"
        "{% endwith %}{% endlanguage %}{{ n }}{% endwith %}",
        "{% language 'de' %}{% include 'row.html' only %}{% endlanguage %}"
        "{% include 'row.html' only %}{% language 'fr' %}{% include 'row.html' %}"
        "{% endlanguage %}",
        "{% for i in rows %}{% with a=i %}{% if forloop.first %}{% language 'de' %}"
        "{{ n }}{% endlanguage %}{% endif %}{{ n }}{% endwith %}{% endfor %}",
        "{% language 'de' %}{% for i in rows %}{% with a=i %}{{ n }}{% endwith %}"
        "{% endfor %}{% endlanguage %}{{ n }}",
    ],
)
def test_shared_formats_follow_language_blocks(source):
    """Scopes share one format cell, but `{% language %}` gets its own (#368)."""
    options = {
        "libraries": {"i18n": "django.templatetags.i18n"},
        "loaders": [
            ("django.template.loaders.locmem.Loader", {"row.html": "[{{ 7654321 }}]"})
        ],
    }
    source = "{% load i18n %}" + source
    django = DjangoEngine(**options).from_string(source)
    rusty = Engine(**options).from_string(source)
    context = {"n": 1234567, "rows": [1, 2, 3]}
    with override_settings(USE_THOUSAND_SEPARATOR=True):
        for _ in range(2):
            expected = django.render(Context(dict(context)))
            assert rusty.render(dict(context)) == expected
    assert "." in expected and "," in expected


DATE_SOURCE = (
    "{{ d|date }}|{{ d|date:'D, j M Y' }}|{{ d|date:'SHORT_DATE_FORMAT' }}|"
    "{{ d|time }}|{{ d|date:'N a A' }}|{{ d }}"
)


def test_date_names_and_formats_follow_language_and_settings():
    import datetime

    context = {"d": datetime.datetime(2026, 3, 1, 0, 0, tzinfo=datetime.UTC)}
    django = DjangoEngine().from_string(DATE_SOURCE)
    rusty = Engine().from_string(DATE_SOURCE)
    seen = set()
    for language in ["en", "de", "fr", "en", "xx", "de", "ja", "en"]:
        for step in [{}, {"DATE_FORMAT": "Y/m/d"}, {"TIME_FORMAT": "H:i:s"}, {}]:
            with translation.override(language), override_settings(**step):
                expected = django.render(Context(dict(context)))
                assert rusty.render(dict(context)) == expected
                seen.add(expected)
    assert len(seen) > 4


def test_date_names_with_translations_off():
    import datetime

    context = {"d": datetime.datetime(2026, 3, 1, 12, 0, tzinfo=datetime.UTC)}
    django = DjangoEngine().from_string(DATE_SOURCE)
    rusty = Engine().from_string(DATE_SOURCE)
    for use_i18n in [True, False, True]:
        with override_settings(USE_I18N=use_i18n), translation.override("de"):
            expected = django.render(Context(dict(context)))
            assert rusty.render(dict(context)) == expected
