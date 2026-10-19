from datetime import datetime
from zoneinfo import ZoneInfo

import pytest
from django.template import Context, Engine as DjangoEngine
from django.utils import timezone, translation
from django_rusty_templates import Engine


@pytest.mark.parametrize("autoescape", [False, True])
@pytest.mark.parametrize("use_l10n", [None, False, True])
@pytest.mark.parametrize("use_tz", [None, False, True])
def test_numeric_and_date_formatting_flag_combinations(autoescape, use_l10n, use_tz):
    source = "{% for value in values %}{{ value }}|{% endfor %}{{ 1234.5 }}"
    values = [1234, 1234.5, datetime(2026, 10, 12, 3, tzinfo=ZoneInfo("UTC"))]
    with translation.override("de"), timezone.override(ZoneInfo("Asia/Tokyo")):
        output = []
        for cls in (DjangoEngine, Engine):
            context = Context(
                {"values": values},
                autoescape=autoescape,
                use_l10n=use_l10n,
                use_tz=use_tz,
            )
            output.append(cls().from_string(source).render(context))
        assert output[0] == output[1]


class HtmlValue:
    def __str__(self):
        return "<value>"


def test_formatting_adapters_do_not_leak_autoescape_between_scopes():
    source = (
        "{{ value }}|{% autoescape off %}{{ value }}{% endautoescape %}|{{ value }}"
    )
    for cls in (DjangoEngine, Engine):
        template = cls().from_string(source)
        assert template.render(Context({"value": HtmlValue()})) == (
            "&lt;value&gt;|<value>|&lt;value&gt;"
        )


NUMBERS = [
    0,
    7,
    -7,
    999,
    1000,
    -1000,
    1234567,
    -1234567890123,
    10**30,
    True,
    False,
    0.0,
    -0.0,
    0.5,
    1234.5,
    -1234567.891,
    12345678.125,
    1e16,
    -1.5e-7,
    float("inf"),
    float("-inf"),
    float("nan"),
]

NUMBER_TEMPLATES = [
    "{% for value in values %}{{ value }}|{% endfor %}",
    "{% autoescape off %}{% for value in values %}{{ value }}|{% endfor %}{% endautoescape %}",
    # Tags that capture a value's output as a string.
    "{% for value in values %}{% firstof value %}|{% cycle value value %}|{% endfor %}",
    "{% load l10n %}{% for value in values %}{% localize off %}{{ value }}{% endlocalize %}|"
    "{% localize on %}{{ value }}{% endlocalize %}|{% endfor %}",
    '{% load i18n %}{% for value in values %}{{ value }}|{% language "de" %}{{ value }}|'
    '{% language "fr" %}{{ value }}{% endlanguage %}|{{ value }}{% endlanguage %}|'
    "{{ value }}|{% endfor %}",
]


LIBRARIES = {"i18n": "django.templatetags.i18n", "l10n": "django.templatetags.l10n"}


def assert_numbers_match_django(source, values):
    django_template = DjangoEngine(libraries=LIBRARIES).from_string(source)
    expected = django_template.render(Context({"values": values}))
    template = Engine(libraries=LIBRARIES).from_string(source)
    assert template.render({"values": values}) == expected
    assert template.render(Context({"values": values})) == expected


# hi has no NUMBER_GROUPING and ta neither it nor THOUSAND_SEPARATOR, so they
# use the settings below. The quote checks that separators are escaped.
@pytest.mark.parametrize("source", NUMBER_TEMPLATES)
@pytest.mark.parametrize("language", ["en", "de", "fr", "zh-hans", "hi", "ta"])
@pytest.mark.parametrize("use_thousand_separator", [False, True])
def test_number_localization_matches_django(
    settings, source, language, use_thousand_separator
):
    settings.USE_THOUSAND_SEPARATOR = use_thousand_separator
    settings.NUMBER_GROUPING = (3, 2, 0)
    settings.THOUSAND_SEPARATOR = "'"
    with translation.override(language):
        assert_numbers_match_django(source, NUMBERS)


@pytest.mark.parametrize("grouping", [0, 2, -1, (3, 2, 0), (1, 0), (3, -1), [4, 2]])
def test_number_grouping_settings_match_django(settings, grouping):
    settings.USE_THOUSAND_SEPARATOR = True
    settings.NUMBER_GROUPING = grouping
    with translation.override("hi"):
        assert_numbers_match_django(NUMBER_TEMPLATES[0], NUMBERS)
