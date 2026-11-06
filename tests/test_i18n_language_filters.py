"""The language_name* and language_bidi filters match django.templatetags.i18n."""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.test import override_settings
from django.utils import translation
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine


class Code(str):
    pass


replacement = Library()


@replacement.filter(name="language_name")
def not_language_name(value):
    return "not language_name"


module = ModuleType("i18n_language_filters_replacement")
module.register = replacement
sys.modules[module.__name__] = module
LIBRARIES = {"i18n": "django.templatetags.i18n", "fake_i18n": module.__name__}

CODES = ["en", "fr", "de", "ar", "he", "fa", "ja", "zh-hans", "zh-hant", "zh-cn"]
CODES += [
    "pt-br",
    "en-us",
    "en-gb",
    "sr-latn",
    "nb",
    "no",
    "es-mx",
    "de-at",
    "fr",
    "de",
]
VALUES = {
    "codes": CODES,
    "safe": mark_safe("fr"),
    "subclass": Code("de"),
    "unknown": "xx",
    "unknown_region": "xx-yy",
    "empty": "",
    "number": 1,
    "none": None,
}
FILTERS = [
    "language_name",
    "language_name_local",
    "language_name_translated",
    "language_bidi",
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


@pytest.mark.parametrize("use_i18n", [True, False])
@pytest.mark.parametrize("language", ["en", "de", "fr", "ar", "pl", "zh-hans"])
@pytest.mark.parametrize("name", FILTERS)
def test_language_filters_for_codes(name, language, use_i18n):
    source = (
        "{% load i18n %}{% for code in codes %}{{ code }}={{ code|"
        + name
        + " }};{% endfor %}{{ 'it'|"
        + name
        + " }}|{{ safe|"
        + name
        + " }}|{{ subclass|"
        + name
        + " }}"
    )
    with override_settings(USE_I18N=use_i18n), translation.override(language):
        django, rusty = render_both(source)
    assert rusty == django


@pytest.mark.parametrize("name", FILTERS)
@pytest.mark.parametrize(
    "value", ["unknown", "unknown_region", "empty", "number", "none", "missing"]
)
def test_unusual_inputs(name, value):
    source = "{% load i18n %}{{ " + value + "|" + name + " }}"
    django, rusty = render_both(source)
    assert rusty == django


@pytest.mark.parametrize(
    "source",
    [
        "{% language 'de' %}{{ 'fr'|language_name_translated }}{% endlanguage %}"
        "{{ 'fr'|language_name_translated }}"
        "{% language 'pl' %}{{ 'fr'|language_name_translated }}{% endlanguage %}",
        "{% get_available_languages as langs %}{% for code, name in langs %}"
        "{{ code|language_name_local }}{{ code|language_bidi }}{% endfor %}",
        "{% if 'ar'|language_bidi %}rtl{% else %}ltr{% endif %}",
        "{{ 'en'|language_name|upper }}|{{ 'en'|language_bidi|yesno:'r,l' }}",
        "{% autoescape off %}{{ 'fr'|language_name_local }}{% endautoescape %}",
        "{% get_language_info for 'de' as info %}{{ info.name }}{{ 'de'|language_name }}",
    ],
)
@pytest.mark.parametrize("language", ["en", "fr"])
def test_language_filters_with_tags(source, language):
    with translation.override(language):
        django, rusty = render_both("{% load i18n %}" + source)
    assert rusty == django


def test_replacement_library_filters_stay_python():
    source = "{% load i18n %}{% load fake_i18n %}{{ 'en'|language_name }}"
    django, rusty = render_both(source)
    assert rusty == django == "not language_name"
