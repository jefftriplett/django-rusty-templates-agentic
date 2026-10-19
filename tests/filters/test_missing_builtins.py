import datetime
from decimal import Decimal

import pytest
from django.template import TemplateSyntaxError, engines
from django.template.defaultfilters import register
from django.utils import translation
from django.utils.safestring import mark_safe


def render_both(source, context):
    """Render with Django and Rusty, returning output or the exception raised."""
    results = []
    for name in ("django", "rusty"):
        try:
            results.append(engines[name].from_string(source).render(context))
        except Exception as error:
            results.append((type(error), str(error)))
    return results


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


TRUNCATECHARS_LENGTHS = [
    -1,
    0,
    1,
    2,
    3,
    4,
    5,
    9,
    10,
    11,
    12,
    "5",
    "-1",
    "x",
    " 5",
    "1_0",
    2.5,
]


@pytest.mark.parametrize("language", ["en", "ckb", "he", "de"])
@pytest.mark.parametrize(
    "value",
    [
        pytest.param("", id="empty"),
        pytest.param("abcdefghij", id="ten_ascii"),
        pytest.param("a <b> & 'c'", id="html"),
        pytest.param(mark_safe("<b>bold</b>"), id="safe"),
        pytest.param("héllo wörld", id="non_ascii"),
        pytest.param("e\u0301" * 6, id="combining"),
        pytest.param("日本語のテキスト", id="wide"),
        pytest.param(translation.gettext_lazy("Goodbye"), id="lazy"),
        pytest.param(12345678901, id="int"),
        pytest.param(Decimal("1234567.891"), id="decimal"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_truncatechars_matches_django(language, value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "{% for n in lengths %}[{{ value|truncatechars:n }}]{% endfor %}"
        '[{{ missing|truncatechars:3 }}][{{ value|truncatechars:"3" }}]'
        "{% endautoescape %}"
    )
    context = {"value": value, "lengths": TRUNCATECHARS_LENGTHS}
    with translation.override(language):
        django, rusty = render_both(source, context)
    assert rusty == django


@pytest.mark.parametrize(
    "text",
    [
        pytest.param("%(truncated_text)s", id="empty"),
        pytest.param("%(truncated_text)s [more]", id="long"),
        pytest.param("[more] %(truncated_text)s", id="prefix"),
        pytest.param("%(truncated_text)s\u0301\u2026", id="combining"),
        pytest.param("\u2026", id="no_placeholder"),
        pytest.param("%(truncated_text)s%%", id="escaped_percent"),
        pytest.param("%(truncated_text)s|%(truncated_text)s", id="twice"),
    ],
)
def test_truncatechars_translated_text(monkeypatch, text):
    pgettext = translation._trans.pgettext

    def translate(context, message):
        if context == "String to return when truncating text":
            return text
        return pgettext(context, message)

    monkeypatch.setattr(translation._trans, "pgettext", translate)
    source = "{% for n in lengths %}[{{ value|truncatechars:n }}]{% endfor %}"
    for value in ["abcdefghij", "abc\u2026", "héllo wörld"]:
        context = {"value": value, "lengths": TRUNCATECHARS_LENGTHS}
        django, rusty = render_both(source, context)
        assert rusty == django


def test_truncatechars_follows_language_changes():
    def activate():
        translation.activate("ckb")
        return ""

    source = (
        "{% load i18n %}{{ value|truncatechars:5 }}"
        "{% language 'he' %}{{ value|truncatechars:5 }}{% endlanguage %}"
        "{{ value|truncatechars:5 }}{{ activate }}{{ value|truncatechars:5 }}"
    )
    context = {"value": "abcdefghij", "activate": activate}
    with translation.override("en"):
        django = engines["django"].from_string(source).render(context)
    with translation.override("en"):
        rusty = engines["rusty"].from_string(source).render(context)
    assert rusty == django == "abcd…abc\u202e…abcd…ab..."


@pytest.mark.parametrize(
    "length", [None, [5], "", "5.0"], ids=["none", "list", "empty", "float_text"]
)
def test_truncatechars_invalid_length(length):
    django, rusty = render_both(
        "{{ value|truncatechars:length }}", {"value": "abcdefghij", "length": length}
    )
    assert rusty == django


def test_truncatechars_replaced_builtin(monkeypatch):
    monkeypatch.setitem(
        register.filters, "truncatechars", lambda value, length: "replacement"
    )
    django, rusty = render_both("{{ value|truncatechars:5 }}", {"value": "abcdefghij"})
    assert rusty == django == "replacement"


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
