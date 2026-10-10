import datetime

import pytest
from django.template import TemplateSyntaxError
from django.utils.safestring import mark_safe


@pytest.mark.parametrize(
    "expression,value,expected",
    [
        ('join:","', ["a", "b"], "a,b"),
        ('join:"&"', ["<a>", "b"], "&lt;a&gt;&b"),
        ("join:separator", [mark_safe("<a>"), "b"], "<a>&amp;b"),
        ("time", datetime.time(13, 30), "1:30 p.m."),
        ('time:"H:i"', datetime.time(13, 30), "13:30"),
        ("unordered_list", ["a", "b"], "\t<li>a</li>\n\t<li>b</li>"),
        ("make_list|join:','", 123, "1,2,3"),
        ("truncatechars:5", "abcdef", "abcd…"),
        ("escapeseq|join:','", ["<a>", "&b"], "&lt;a&gt;,&amp;b"),
        ("iriencode", "café", "caf%C3%A9"),
        ("truncatewords:2", "one two three", "one two …"),
        ("random", ["only"], "only"),
        ("phone2numeric", "1-FLOWERS", "1-3569377"),
        ("ljust:4", "ab", "ab  "),
        ("rjust:4", "ab", "  ab"),
        ("slice:'1:3'|join:','", [0, 1, 2, 3], "1,2"),
        ("stringformat:'03d'", 7, "007"),
        ("striptags", "<b>hello</b>", "hello"),
        ("linebreaks", "one\ntwo", "<p>one<br>two</p>"),
        ("linebreaksbr", "one\ntwo", "one<br>two"),
        ("linenumbers", "one\ntwo", "1. one\n2. two"),
        ("urlencode", "a b/c", "a%20b/c"),
        ("floatformat", 1.234, "1.2"),
        ("floatformat:2", 1.234, "1.23"),
        ("first", ["a", "b"], "a"),
        ("first", [], ""),
        (
            "json_script:'data'",
            {"key": "<tag>"},
            '<script id="data" type="application/json">{"key": "\\u003Ctag\\u003E"}</script>',
        ),
    ],
)
def test_builtin_rendering(assert_render, expression, value, expected):
    assert_render(
        "{{ value|" + expression + " }}", {"value": value, "separator": "&"}, expected
    )


def test_join_autoescape_off(assert_render):
    assert_render(
        '{% autoescape off %}{{ value|join:"&" }}{% endautoescape %}',
        {"value": ["<a>", "b"]},
        "<a>&b",
    )


def test_safe_input_preserved(assert_render):
    assert_render(
        "{{ value|truncatechars:10 }}",
        {"value": mark_safe("<b>hello</b>")},
        "<b>hello<…",
    )


@pytest.mark.parametrize(
    "name",
    [
        "join",
        "ljust",
        "rjust",
        "slice",
        "stringformat",
        "truncatechars",
        "truncatewords",
    ],
)
def test_required_argument(template_engine, name):
    with pytest.raises(TemplateSyntaxError):
        template_engine.from_string("{{ value|" + name + " }}")


@pytest.mark.parametrize(
    "name", ["first", "make_list", "random", "striptags", "phone2numeric", "escapeseq"]
)
def test_unexpected_argument(template_engine, name):
    with pytest.raises(TemplateSyntaxError):
        template_engine.from_string("{{ value|" + name + ":2 }}")
