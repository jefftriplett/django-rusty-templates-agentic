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


@pytest.mark.parametrize("name", ["truncatechars", "truncatewords"])
@pytest.mark.parametrize(
    "length", [None, [5], "", "5.0"], ids=["none", "list", "empty", "float_text"]
)
def test_truncate_invalid_length(name, length):
    django, rusty = render_both(
        "{{ value|" + name + ":length }}",
        {"value": "abc def ghi jkl mno", "length": length},
    )
    assert rusty == django


@pytest.mark.parametrize("name", ["truncatechars", "truncatewords"])
def test_truncate_replaced_builtin(monkeypatch, name):
    monkeypatch.setitem(register.filters, name, lambda value, length: "replacement")
    django, rusty = render_both(
        "{{ value|" + name + ":5 }}", {"value": "abc def ghi jkl mno"}
    )
    assert rusty == django == "replacement"


SLICE_ARGUMENTS = [
    "",
    ":",
    "::",
    "2",
    ":2",
    "-1",
    "1:-1",
    "-3:",
    "100:",
    "-100:100",
    "::2",
    "::-1",
    "5:1:-1",
    "-1:-6:-2",
    "::0",
    "1:2:3:4",
    "+2",
    " 2",
    "1_0",
    "a",
    "1.5",
    "99999999999999999999",
]


class SliceRecorder:
    def __getitem__(self, key):
        return f"<{key!r}>"


class SliceIndexError:
    def __getitem__(self, key):
        raise IndexError("no items")


@pytest.mark.parametrize(
    "value",
    [
        pytest.param("a <b> & 'c' d", id="ascii"),
        pytest.param("héllo wörld ✓ 😀", id="non_ascii"),
        pytest.param(mark_safe("<b>bold</b> ü"), id="safe"),
        pytest.param(["a", "<b>", "c", "d", "e"], id="list"),
        pytest.param((1, 2, 3, 4, 5, 6), id="tuple"),
        pytest.param(range(10), id="range"),
        pytest.param({"a": 1}, id="dict"),
        pytest.param(SliceRecorder(), id="getitem"),
        pytest.param(12345, id="int"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_slice_matches_django(value, autoescape):
    # Context strings reach the filter as Python objects; `lower` and `safe`
    # hand it native plain and safe strings.
    literals = "".join(
        "[{{ value" + prefix + "|slice:'" + argument + "' }}]"
        for argument in SLICE_ARGUMENTS
        for prefix in ["", "|lower", "|safe"]
    )
    source = (
        "{% autoescape "
        + autoescape
        + " %}"
        + literals
        + "{% for argument in arguments %}"
        "[{{ value|slice:argument }}][{{ value|lower|slice:argument }}]"
        "{% endfor %}"
        "[{{ value|slice:2 }}][{{ value|slice:-1 }}][{{ value|slice:1.5 }}]"
        "[{{ missing|slice:':2' }}]"
        "[{{ value|slice:'1:-1'|upper }}][{{ value|slice:'::-1'|length }}]"
        "{% endautoescape %}"
    )
    arguments = [*SLICE_ARGUMENTS, 2, -1, 0, 10**30, 1.5, True, None, ["x"]]
    django, rusty = render_both(source, {"value": value, "arguments": arguments})
    assert rusty == django


def test_slice_propagates_other_errors():
    django, rusty = render_both("{{ value|slice:':2' }}", {"value": SliceIndexError()})
    assert rusty == django == (IndexError, "no items")


def test_slice_replaced_builtin(monkeypatch):
    monkeypatch.setitem(register.filters, "slice", lambda value, arg: "replacement")
    django, rusty = render_both("{{ value|slice:':2' }}", {"value": "abcdefghij"})
    assert rusty == django == "replacement"


TRUNCATEWORDS_LENGTHS = [
    -1,
    0,
    1,
    2,
    3,
    4,
    10,
    "2",
    "-1",
    "x",
    " 2",
    "1_0",
    2.5,
    10**30,
]


@pytest.mark.parametrize("language", ["en", "he"])
@pytest.mark.parametrize(
    "value",
    [
        pytest.param("", id="empty"),
        pytest.param("one", id="one_word"),
        pytest.param(" \t\n ", id="only_whitespace"),
        pytest.param("one two three four", id="ascii"),
        pytest.param("  one \t two\n\nthree  ", id="collapsed_whitespace"),
        pytest.param("one\r\ntwo\x0bthree\x0cfour", id="control_whitespace"),
        pytest.param("one\xa0two\u3000three\x1cfour\x85five", id="unicode_whitespace"),
        pytest.param("one\u200btwo three", id="zero_width_space"),
        pytest.param("a <b> & 'c' \"d\"", id="html"),
        pytest.param(mark_safe("<b>bold</b> and <i>italic</i> text"), id="safe"),
        pytest.param("héllo wörld 日本語 e\u0301", id="non_ascii"),
        pytest.param("a … b … c", id="ellipsis_words"),
        pytest.param("a…  …", id="ellipsis_suffix"),
        pytest.param(translation.gettext_lazy("Good morning"), id="lazy"),
        pytest.param(12345678901, id="int"),
        pytest.param(Decimal("1234567.891"), id="decimal"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_truncatewords_matches_django(language, value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "{% for n in lengths %}[{{ value|truncatewords:n }}]{% endfor %}"
        '[{{ missing|truncatewords:3 }}][{{ value|truncatewords:"2" }}]'
        "{% endautoescape %}"
    )
    context = {"value": value, "lengths": TRUNCATEWORDS_LENGTHS}
    with translation.override(language):
        django, rusty = render_both(source, context)
    assert rusty == django


def test_truncatewords_splits_like_python():
    every_character = "".join(
        f"x{chr(code)}" for code in range(0x110000) if not 0xD800 <= code <= 0xDFFF
    )
    source = "{{ value|truncatewords:n }}"
    for n in [10**6, 3]:
        django, rusty = render_both(source, {"value": every_character, "n": n})
        assert rusty == django


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


FLOATFORMAT_VALUES = {
    "negative": [
        -1,
        -0.0,
        -0.4,
        -0.5,
        -1.25,
        -1234567.891,
        Decimal("-0"),
        Decimal("-0.005"),
    ],
    "half": [
        0.5,
        1.5,
        2.5,
        0.05,
        0.15,
        1.005,
        2.675,
        9.995,
        999.995,
        Decimal("2.5"),
        Decimal("0.125"),
    ],
    "large": [
        1234567,
        10**15,
        1e16,
        1e21,
        1.5e300,
        10**30,
        10**199,
        10**250,
        12345678.125,
    ],
    "small": [1e-7, 0.0004, 1.23e-5, 5e-324, Decimal("1.2E-7"), Decimal("0E-10")],
    "decimal": [
        Decimal("34.23234"),
        Decimal("34.00000"),
        Decimal("1E+2"),
        Decimal("123456.7890"),
        Decimal("9.9999"),
    ],
    "invalid": [
        None,
        "",
        "abc",
        "1.5",
        True,
        [1],
        float("inf"),
        float("nan"),
        Decimal("NaN"),
        Decimal("-Infinity"),
    ],
}
FLOATFORMAT_PRECISIONS = [
    0,
    1,
    2,
    3,
    -1,
    -2,
    -3,
    30,
    "2",
    "-2",
    "0",
    "abc",
    "1.5",
    "g",
    "2g",
    "-2g",
    "u",
    "2u",
    "gu",
    "2ug",
]


@pytest.mark.parametrize("thousand_separator", [False, True], ids=["plain", "grouped"])
@pytest.mark.parametrize("language", ["en", "de", "fr"])
@pytest.mark.parametrize("values", FLOATFORMAT_VALUES.values(), ids=FLOATFORMAT_VALUES)
def test_floatformat_matches_django(settings, values, language, thousand_separator):
    settings.USE_THOUSAND_SEPARATOR = thousand_separator
    source = (
        "{% for v in values %}[{{ v|floatformat }}]"
        "{% for p in precisions %}[{{ v|floatformat:p }}]{% endfor %}{% endfor %}"
    )
    context = {"values": values, "precisions": FLOATFORMAT_PRECISIONS}
    with translation.override(language):
        django, rusty = render_both(source, context)
    assert rusty == django


def test_floatformat_literals(settings):
    settings.USE_THOUSAND_SEPARATOR = True
    source = (
        "{{ 1234567|floatformat:2 }} {{ -2.5|floatformat:0 }} {{ 1.5|floatformat }} "
        '{{ 0.125|floatformat:"2" }} {{ 2|floatformat:"-2" }}'
    )
    django, rusty = render_both(source, {})
    assert rusty == django == "1,234,567.00 -3 1.5 0.13 2"


@pytest.mark.parametrize(
    "grouping", [(3, 2, 0), (3, 0), (1,), (2, 0, 1), 0, 2, (-1, 3), ()], ids=repr
)
def test_floatformat_number_grouping(settings, grouping):
    settings.USE_THOUSAND_SEPARATOR = True
    settings.NUMBER_GROUPING = grouping
    settings.THOUSAND_SEPARATOR = "&"
    settings.DECIMAL_SEPARATOR = ",,"
    # Afrikaans has no format module, so these settings apply.
    source = (
        "{% for v in values %}[{{ v|floatformat:2 }}][{{ v|floatformat }}]{% endfor %}"
    )
    context = {"values": [0, -12345678, 1234567.891, Decimal("98765432.1")]}
    with translation.override("af"):
        django, rusty = render_both(source, context)
    assert rusty == django


def test_floatformat_follows_language_changes(settings):
    settings.USE_THOUSAND_SEPARATOR = True

    def activate():
        translation.activate("de")
        return ""

    source = (
        "{% load i18n %}{{ value|floatformat:2 }} "
        "{% language 'fr' %}{{ value|floatformat:2 }}{% endlanguage %} "
        "{{ value|floatformat:2 }}{{ activate }} {{ value|floatformat:2 }}"
    )
    context = {"value": 1234.5, "activate": activate}
    with translation.override("en"):
        django = engines["django"].from_string(source).render(context)
    with translation.override("en"):
        rusty = engines["rusty"].from_string(source).render(context)
    assert rusty == django == "1,234.50 1\xa0234,50 1,234.50 1.234,50"


@pytest.mark.parametrize(
    "precision",
    [None, [2], "", "1.5", 2.5],
    ids=["none", "list", "empty", "float_text", "float"],
)
def test_floatformat_invalid_precision(precision):
    django, rusty = render_both(
        "{{ value|floatformat:precision }}", {"value": 1.25, "precision": precision}
    )
    assert rusty == django


def test_floatformat_replaced_builtin(monkeypatch):
    monkeypatch.setitem(
        register.filters, "floatformat", lambda value, precision=-1: "replacement"
    )
    django, rusty = render_both("{{ value|floatformat:2 }}", {"value": 1.5})
    assert rusty == django == "replacement"


class JoinHtml:
    def __html__(self):
        return "<i>html</i>"

    def __repr__(self):
        return "JoinHtml"


class JoinStr:
    def __str__(self):
        return "<str & obj>"

    def __repr__(self):
        return "JoinStr"


class JoinHtmlNotString:
    def __html__(self):
        return 1

    def __repr__(self):
        return "JoinHtmlNotString"


class JoinStrTypeError:
    def __str__(self):
        raise TypeError("no str")

    def __repr__(self):
        return "JoinStrTypeError"


class JoinStrSubclass(str):
    pass


JOIN_VALUES = {
    "strings": lambda: ["a", "<b>", "c & 'd'", '"e"'],
    "empty": lambda: [],
    "single": lambda: ["<only>"],
    "tuple": lambda: ("<a>", "b"),
    "safe_items": lambda: [mark_safe("<b>bold</b>"), "<i>"],
    "ints": lambda: [1, -2, 10**30],
    "mixed": lambda: [1.5, True, None, Decimal("2.50"), "<x>"],
    "nested": lambda: [["<a>", 1], ("b",)],
    "lazy": lambda: [translation.gettext_lazy("Goodbye"), "<x>"],
    "html_object": lambda: [JoinHtml(), "<x>"],
    "str_object": lambda: [JoinStr()],
    "html_not_string": lambda: ["a", JoinHtmlNotString()],
    "str_type_error": lambda: ["a", JoinStrTypeError()],
    "str_subclass": lambda: [JoinStrSubclass("<sub>"), "b"],
    "generator": lambda: (item for item in ["<a>", "b"]),
    "range": lambda: range(3),
    "dict": lambda: {"<k>": 1, "v": 2},
    "string": lambda: "a<b",
    "safe_string": lambda: mark_safe("<b>"),
    "int": lambda: 5,
    "none": lambda: None,
}

JOIN_SEPARATORS = {
    "literal": '","',
    "literal_html": '"<br>"',
    "translated": '_("<sep>")',
    "variable": "separator",
    "safe_variable": "safe_separator",
    "int_variable": "int_separator",
    "none_variable": "none_separator",
}


@pytest.mark.parametrize("autoescape", ["on", "off"])
@pytest.mark.parametrize("separator", JOIN_SEPARATORS.values(), ids=JOIN_SEPARATORS)
@pytest.mark.parametrize("value", JOIN_VALUES.values(), ids=JOIN_VALUES)
def test_join_matches_django(value, separator, autoescape):
    source = (
        "{% load i18n %}{% autoescape " + autoescape + " %}"
        "[{{ value|join:" + separator + " }}][{{ missing|join:" + separator + " }}]"
        "{% endautoescape %}"
    )
    results = []
    for name in ("django", "rusty"):
        context = {
            "value": value(),
            "separator": "<&>",
            "safe_separator": mark_safe("<hr>"),
            "int_separator": 1,
            "none_separator": None,
        }
        try:
            results.append(engines[name].from_string(source).render(context))
        except Exception as error:
            results.append((type(error), str(error)))
    django, rusty = results
    assert rusty == django


def test_join_replaced_builtin(monkeypatch):
    monkeypatch.setitem(register.filters, "join", lambda value, arg: "replacement")
    django, rusty = render_both('{{ value|join:"," }}', {"value": ["a", "b"]})
    assert rusty == django == "replacement"
