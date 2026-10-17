"""Filters whose upstream tests call Python functions instead of templates."""

import pytest
from django.template import Context, Engine as DjangoEngine, TemplateSyntaxError
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("autoescape", [False, True])
@pytest.mark.parametrize(
    "expression,value,argument",
    [
        ("truncatechars_html:arg", "<p>Hello <b>world</b>!</p>", 8),
        ("truncatechars_html:arg", mark_safe("<p>Already safe &amp; long</p>"), 10),
        ("truncatechars_html:arg", "<p>Short</p>", 100),
        ("truncatechars_html:arg", "<p>Invalid limit</p>", "invalid"),
        ("truncatewords_html:arg", "<p>One <b>two three</b> four</p>", 2),
        ("truncatewords_html:arg", "<p>One &amp; two</p>", 0),
        ("truncatewords_html:arg", "<p>One two</p>", "invalid"),
        ("dictsort:arg", [{"name": "Zulu"}, {"name": "Alpha"}], "name"),
        ("dictsortreversed:arg", [{"name": "Zulu"}, {"name": "Alpha"}], "name"),
        (
            "dictsort:arg",
            [{"person": {"age": 2}}, {"person": {"age": 1}}],
            "person.age",
        ),
        ("dictsort:arg", [("b", 2), ("a", 1)], 0),
        ("dictsort:arg", [{"name": "A"}], "missing"),
        ("dictsortreversed:arg", [], "name"),
        ("get_digit:arg", 123456, 2),
        ("get_digit:arg", 123456, 20),
        ("get_digit:arg", 123456, 0),
        ("get_digit:arg", "invalid", 2),
        ("filesizeformat", 0, None),
        ("filesizeformat", 1024, None),
        ("filesizeformat", 1024**3, None),
        ("filesizeformat", -1024, None),
        ("filesizeformat", "invalid", None),
        ("pprint", {"unsafe": "<script>", "values": [1, 2]}, None),
        ("pprint", None, None),
        ("pprint", mark_safe("<p>safe input</p>"), None),
    ],
)
def test_filter_output_matches_django(debug, autoescape, expression, value, argument):
    source = "{{ value|" + expression + " }}"
    values = {"value": value, "arg": argument}
    expected = (
        DjangoEngine(debug=debug)
        .from_string(source)
        .render(Context(values, autoescape=autoescape))
    )
    template = Engine(debug=debug).from_string(source)
    assert template._is_native
    assert template.render(Context(values, autoescape=autoescape)) == expected


@pytest.mark.parametrize(
    "expression",
    [
        "truncatechars_html",
        "truncatewords_html",
        "dictsort",
        "dictsortreversed",
        "get_digit",
        "filesizeformat:arg",
        "pprint:arg",
    ],
)
def test_invalid_filter_arity_matches_django(expression):
    source = "{{ value|" + expression + " }}"
    with pytest.raises(TemplateSyntaxError) as reference:
        DjangoEngine().from_string(source)
    with pytest.raises(TemplateSyntaxError) as rusty:
        Engine().from_string(source)
    assert str(rusty.value) == str(reference.value)
