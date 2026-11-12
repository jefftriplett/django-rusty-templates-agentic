"""A filter on a missing variable sees Django's value for it (#376): the
empty `string_if_invalid` in output and tag arguments, `None` where failures
are ignored, as in `{% if %}`."""

import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine

FILTERS = [
    "add:text",
    "add:1",
    "center:1",
    "center:text",
    "ljust:3",
    "wordwrap:text",
    "cut:1",
    'cut:"a"',
    "length",
    "length_is:0",
    "first",
    "last",
    "upper",
    "capfirst",
    "title",
    "slugify",
    "default:'d'",
    "default_if_none:'d'",
    "yesno",
    'yesno:"a,b"',
    "pluralize",
    "floatformat",
    "filesizeformat",
    "wordcount",
    "make_list",
    "stringformat:'s'",
    "date",
    "urlencode",
    "linebreaksbr",
    "join:','",
    "divisibleby:2",
    "get_digit:1",
    "truncatechars:2",
    "slice:':1'",
    "escape",
    "safe",
    "striptags",
]


def outcome(engine, source, context):
    try:
        return engine.from_string(source).render(context)
    except Exception as error:
        return type(error)


@pytest.mark.parametrize("name", FILTERS)
@pytest.mark.parametrize(
    "source",
    [
        "{{{{ missing|{} }}}}",
        "{{% if missing|{} %}}yes{{% else %}}no{{% endif %}}",
        "{{% with value=missing|{} %}}[{{{{ value }}}}]{{% endwith %}}",
        "{{% for x in missing|{} %}}{{{{ x }}}},{{% endfor %}}",
    ],
)
def test_filter_on_missing_value_matches_django(name, source):
    source = source.format(name)
    expected = outcome(DjangoEngine(), source, Context({"text": "abc"}))
    assert outcome(Engine(), source, {"text": "abc"}) == expected


@pytest.mark.parametrize("name", FILTERS)
def test_string_if_invalid_skips_filters(name):
    source = f"{{{{ missing|{name} }}}}"
    django = DjangoEngine(string_if_invalid="INVALID %s")
    rusty = Engine(string_if_invalid="INVALID %s")
    expected = outcome(django, source, Context({"text": "abc"}))
    assert outcome(rusty, source, {"text": "abc"}) == expected


@pytest.mark.parametrize(
    "source",
    [
        "{{ text|cut:1 }}",
        "{{ text|cut:number }}",
        "{{ text|cut:items }}",
        "{{ text|cut:safe }}",
        "{{ text|yesno:1 }}",
        "{{ text|yesno:items }}",
        "{{ text|yesno:none }}",
        "{{ none|yesno:none }}",
        "{% for x in none %}{{ x }}{% empty %}empty{% endfor %}",
        "{% for x in none|default_if_none:none %}{{ x }}{% endfor %}",
        "{{ 1e-7 }}",
        "{{ 1e+7 }}",
        "{{ -1e-7 }}",
        "{{ 1.5e-3|add:1 }}",
        "{{ 5.2e3 }}",
        "{{ text|default:1e-7 }}",
    ],
)
def test_argument_types_and_literals_match_django(source):
    from django.utils.safestring import mark_safe

    def context():
        return {
            "text": "abc",
            "number": 1,
            "items": [1],
            "safe": mark_safe("b"),
            "none": None,
        }

    expected = outcome(DjangoEngine(), source, Context(context()))
    assert outcome(Engine(), source, context()) == expected
