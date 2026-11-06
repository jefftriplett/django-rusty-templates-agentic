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
        pytest.param("\u0958\u0344abcdefghij", id="nfc_expands"),
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


@pytest.mark.parametrize(
    "value,expected",
    [
        ("abcdefghij", "abcd…abc\u202e…abcd…ab...abcd…"),
        ("héllo wörld", "héll…hél\u202e…héll…hé...héll…"),
    ],
)
def test_truncatechars_follows_language_changes(value, expected):
    def activate():
        translation.activate("ckb")
        return ""

    def deactivate():
        translation.deactivate_all()
        return ""

    source = (
        "{% load i18n %}{{ value|truncatechars:5 }}"
        "{% language 'he' %}{{ value|truncatechars:5 }}{% endlanguage %}"
        "{{ value|truncatechars:5 }}{{ activate }}{{ value|truncatechars:5 }}"
        "{{ deactivate }}{{ value|truncatechars:5 }}"
    )
    context = {"value": value, "activate": activate, "deactivate": deactivate}
    with translation.override("en"):
        django = engines["django"].from_string(source).render(context)
    with translation.override("en"):
        rusty = engines["rusty"].from_string(source).render(context)
    assert rusty == django == expected


@pytest.mark.parametrize(
    "name", ["truncatechars", "truncatewords", "truncatechars_html"]
)
@pytest.mark.parametrize(
    "length", [None, [5], "", "5.0"], ids=["none", "list", "empty", "float_text"]
)
def test_truncate_invalid_length(name, length):
    django, rusty = render_both(
        "{{ value|" + name + ":length }}",
        {"value": "abc def ghi jkl mno", "length": length},
    )
    assert rusty == django


@pytest.mark.parametrize(
    "name", ["truncatechars", "truncatewords", "truncatechars_html"]
)
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


class HtmlOnly:
    """Has `__html__` but is not `SafeData`, so `str()` loses its safety."""

    def __str__(self):
        return "<i>a</i>\nb"

    def __html__(self):
        return "<i>a</i>"


class SafeStr:
    """`str()` returns a `SafeString`, which `@stringfilter` keeps safe."""

    def __str__(self):
        return mark_safe("<i>a</i>\r\nb")


@pytest.mark.parametrize(
    "value",
    [
        pytest.param("", id="empty"),
        pytest.param("plain", id="plain"),
        pytest.param("one\ntwo", id="lf"),
        pytest.param("a\r\nb\rc\n\rd\r", id="mixed_newlines"),
        pytest.param("\r\r\n\n\n", id="only_newlines"),
        pytest.param("a\n\nb\n\n\nc", id="paragraphs"),
        pytest.param("a\r\n\r\nb\r\rc\n\rd\r\n\ne", id="paragraphs_mixed"),
        pytest.param("\n\na\nb\n\n", id="paragraphs_at_edges"),
        pytest.param("a\n \nb\n\t\n\nc", id="whitespace_line"),
        pytest.param("<b>&\"'</b>\nx", id="html"),
        pytest.param("&amp;\r\n&lt;", id="entities"),
        pytest.param("café\nü<ñ>\r日本", id="non_ascii"),
        pytest.param(mark_safe("<b>x</b>\r\ny"), id="safe"),
        pytest.param(translation.gettext_lazy("Goodbye"), id="lazy"),
        pytest.param(123, id="int"),
        pytest.param(1.0, id="float"),
        pytest.param(1e16, id="float_exponent"),
        pytest.param(True, id="bool"),
        pytest.param(None, id="none"),
        pytest.param(Decimal("1.50"), id="decimal"),
        pytest.param(["<a>", "b\nc"], id="list"),
        pytest.param(HtmlOnly(), id="html_only"),
        pytest.param(SafeStr(), id="safe_str"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
@pytest.mark.parametrize("name", ["linebreaks", "linebreaksbr"])
def test_linebreaks_matches_django(name, value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ value|NAME }}][{{ value|NAME|escape }}]"
        "[{{ value|lower|NAME }}][{{ value|force_escape|NAME }}]"
        "[{{ value|NAME|truncatechars:6 }}][{{ missing|NAME }}]"
        "{% endautoescape %}"
    ).replace("NAME", name)
    django, rusty = render_both(source, {"value": value})
    assert rusty == django


@pytest.mark.parametrize("autoescape", ["on", "off"])
@pytest.mark.parametrize("name", ["linebreaks", "linebreaksbr"])
def test_linebreaks_literals(name, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ '<a>\n\n&'|NAME }}][{{ 1.0|NAME }}]"
        "[{{ 2|NAME }}][{{ 1e20|NAME }}]"
        "{% endautoescape %}"
    ).replace("NAME", name)
    django, rusty = render_both(source, {})
    assert rusty == django


@pytest.mark.parametrize("name", ["linebreaks", "linebreaksbr"])
def test_linebreaks_replaced_builtin(name, monkeypatch):
    monkeypatch.setitem(register.filters, name, lambda value: "replacement")
    django, rusty = render_both("{{ value|" + name + " }}", {"value": "a\nb"})
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


class FirstKeyError:
    def __getitem__(self, key):
        raise KeyError(key)


@pytest.mark.parametrize(
    "value",
    [
        pytest.param("<a&b>", id="string"),
        pytest.param("", id="empty_string"),
        pytest.param("é<", id="non_ascii"),
        pytest.param("😀x", id="astral"),
        pytest.param(mark_safe("<b>"), id="safe_string"),
        pytest.param(["<a>", "b"], id="list"),
        pytest.param([mark_safe("<a>"), "b"], id="safe_item"),
        pytest.param([], id="empty_list"),
        pytest.param((1, 2), id="tuple"),
        pytest.param(range(3, 5), id="range"),
        pytest.param({0: "zero"}, id="dict"),
        pytest.param({"a": 1}, id="dict_missing_key"),
        pytest.param(FirstKeyError(), id="key_error"),
        pytest.param(12, id="int"),
        pytest.param(1.5, id="float"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_first_matches_django(value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ value|first }}][{{ value|lower|first }}][{{ value|safe|first }}]"
        "[{{ missing|first }}][{{ value|first|length }}]"
        "{% endautoescape %}"
    )
    django, rusty = render_both(source, {"value": value})
    assert rusty == django


def test_first_literals():
    source = "[{{ 'abc'|first }}][{{ 12|first }}][{{ ''|first }}]"
    django, rusty = render_both(source, {})
    assert rusty == django


def test_first_replaced_builtin(monkeypatch):
    monkeypatch.setitem(register.filters, "first", lambda value: "replacement")
    django, rusty = render_both("{{ value|first }}", {"value": "abc"})
    assert rusty == django == "replacement"


@pytest.mark.parametrize(
    "value",
    [
        pytest.param("", id="empty"),
        pytest.param("one", id="one_line"),
        pytest.param("a\nb\n", id="trailing_newline"),
        pytest.param("\n".join("x" * n for n in range(12)), id="two_digit_width"),
        pytest.param("\n" * 99, id="hundred_lines"),
        pytest.param("a\r\nb\rc", id="carriage_returns"),
        pytest.param("<a> & 'b' \"c\"\n<d>", id="html"),
        pytest.param(mark_safe("<b>\n<i>"), id="safe"),
        pytest.param("héllo\n日本語 😀", id="non_ascii"),
        pytest.param(12345, id="int"),
        pytest.param(1.5, id="float"),
        pytest.param(True, id="bool"),
        pytest.param(["<a>", "b"], id="list"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_linenumbers_matches_django(value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ value|linenumbers }}][{{ value|safe|linenumbers }}]"
        "[{{ value|lower|linenumbers }}][{{ missing|linenumbers }}]"
        "[{{ value|linenumbers|length }}]"
        "{% endautoescape %}"
    )
    django, rusty = render_both(source, {"value": value})
    assert rusty == django


def test_linenumbers_literals():
    source = "[{{ 'a<\\nb'|linenumbers }}][{{ 12|linenumbers }}]"
    django, rusty = render_both(source, {})
    assert rusty == django


def test_linenumbers_replaced_builtin(monkeypatch):
    monkeypatch.setitem(register.filters, "linenumbers", lambda value: "replacement")
    django, rusty = render_both("{{ value|linenumbers }}", {"value": "a\nb"})
    assert rusty == django == "replacement"


def test_truncation_text_is_cached_for_the_render(monkeypatch):
    # Output is the same with or without the cache, so count the lookups.
    pgettext = translation._trans.pgettext
    calls = []

    def counting(context, message):
        if context == "String to return when truncating text":
            calls.append(message)
        return pgettext(context, message)

    monkeypatch.setattr(translation._trans, "pgettext", counting)
    source = "{% for value in values %}{{ value|truncatechars:3 }}{% endfor %}"
    context = {"values": ["abcdefghij", "héllo wörld"] * 10}
    with translation.override("en"):
        engines["rusty"].from_string(source).render(context)
    assert len(calls) == 1


JUSTIFY_WIDTHS = [-5, 0, 1, 3, 8, "6", "-1", " 4", "1_0", "x", 2.5, True, None, 10**30]


@pytest.mark.parametrize("name", ["ljust", "rjust"])
@pytest.mark.parametrize(
    "value",
    [
        pytest.param("", id="empty"),
        pytest.param("ab", id="ascii"),
        pytest.param("a<b>&", id="html"),
        pytest.param(mark_safe("<b>"), id="safe"),
        pytest.param("é日😀", id="non_ascii"),
        pytest.param("é", id="combining"),
        pytest.param(translation.gettext_lazy("Good morning"), id="lazy"),
        pytest.param(12, id="int"),
        pytest.param(1.5, id="float"),
        pytest.param(["a"], id="list"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_justify_matches_django(name, value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "{% for width in widths %}[{{ value|NAME:width }}]{% endfor %}"
        "[{{ value|NAME:5 }}][{{ value|NAME:'5' }}][{{ value|NAME:-1 }}]"
        "[{{ missing|NAME:3 }}][{{ value|lower|NAME:4 }}][{{ value|NAME:4|length }}]"
        "{% endautoescape %}"
    ).replace("NAME", name)
    context = {"value": value, "widths": JUSTIFY_WIDTHS}
    django, rusty = render_both(source, context)
    assert rusty == django


@pytest.mark.parametrize("name", ["ljust", "rjust"])
def test_justify_huge_width_matches_django(name):
    django, rusty = render_both(
        "{{ value|" + name + ":width }}", {"value": "a", "width": 10**19}
    )
    assert rusty == django


@pytest.mark.parametrize("name", ["ljust", "rjust"])
def test_justify_replaced_builtin(name, monkeypatch):
    monkeypatch.setitem(register.filters, name, lambda value, arg: "replacement")
    django, rusty = render_both("{{ value|" + name + ":4 }}", {"value": "a"})
    assert rusty == django == "replacement"


@pytest.mark.parametrize("language", ["en", "ckb", "he"])
@pytest.mark.parametrize(
    "value",
    [
        pytest.param("", id="empty"),
        pytest.param("abcdefghij", id="ten_ascii"),
        pytest.param("<p>one <b>two</b> three</p>", id="tags"),
        pytest.param("<p>a<br>b<img src='x'/>c</p>", id="void_tags"),
        pytest.param("a &amp; b &lt;c&gt; &#39;d&#39;", id="charrefs"),
        pytest.param("a & b < c ' \"", id="raw_specials"),
        pytest.param("<p>héllo <i>wörld</i></p>", id="non_ascii"),
        pytest.param("e\u0301" * 6, id="combining"),
        pytest.param("<p>unclosed <b>tags", id="unclosed"),
        pytest.param("<!-- comment -->text<script>x<y</script>", id="comment_script"),
        pytest.param(mark_safe("<b>bold</b> text"), id="safe"),
        pytest.param(translation.gettext_lazy("Goodbye"), id="lazy"),
        pytest.param(12345678901, id="int"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_truncatechars_html_matches_django(language, value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "{% for n in lengths %}[{{ value|truncatechars_html:n }}]{% endfor %}"
        '[{{ missing|truncatechars_html:3 }}][{{ value|truncatechars_html:"3" }}]'
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
        pytest.param("%(truncated_text)s<i>…</i>", id="html"),
        pytest.param("\u2026", id="no_placeholder"),
    ],
)
def test_truncatechars_html_translated_text(monkeypatch, text):
    pgettext = translation._trans.pgettext

    def translate(context, message):
        if context == "String to return when truncating text":
            return text
        return pgettext(context, message)

    monkeypatch.setattr(translation._trans, "pgettext", translate)
    source = "{% for n in lengths %}[{{ value|truncatechars_html:n }}]{% endfor %}"
    for value in ["abcdefghij", "<p>abc\u2026</p>", "<b>héllo</b> wörld"]:
        context = {"value": value, "lengths": TRUNCATECHARS_LENGTHS}
        django, rusty = render_both(source, context)
        assert rusty == django


def test_truncatechars_html_follows_language_changes():
    def activate():
        translation.activate("ckb")
        return ""

    def deactivate():
        translation.deactivate_all()
        return ""

    source = (
        "{% load i18n %}{{ value|truncatechars_html:5 }}"
        "{% language 'he' %}{{ value|truncatechars_html:5 }}{% endlanguage %}"
        "{{ activate }}{{ value|truncatechars_html:5 }}"
        "{{ deactivate }}{{ value|truncatechars_html:5 }}"
    )
    context = {
        "value": "<p>abcdefghij</p>",
        "activate": activate,
        "deactivate": deactivate,
    }
    with translation.override("en"):
        django = engines["django"].from_string(source).render(context)
    with translation.override("en"):
        rusty = engines["rusty"].from_string(source).render(context)
    assert rusty == django


class PluralizeSized:
    def __len__(self):
        return 1


PLURALIZE_ARGUMENTS = ["", "es", "y,ies", ",s", "y,", "a,b,c", "<i>,<b>", "x", None, 1]


@pytest.mark.parametrize(
    "value",
    [
        pytest.param(0, id="zero"),
        pytest.param(1, id="one"),
        pytest.param(2, id="two"),
        pytest.param(-1, id="minus_one"),
        pytest.param(10**30, id="big_int"),
        pytest.param(10**400, id="huge_int"),
        pytest.param(1.0, id="float_one"),
        pytest.param(1.5, id="float"),
        pytest.param(float("nan"), id="nan"),
        pytest.param(True, id="true"),
        pytest.param(False, id="false"),
        pytest.param(Decimal("1.0"), id="decimal"),
        pytest.param("1", id="string_one"),
        pytest.param(" 1 ", id="string_spaces"),
        pytest.param("1_0", id="string_underscore"),
        pytest.param("abc", id="string"),
        pytest.param(["a"], id="list_one"),
        pytest.param(["a", "b"], id="list"),
        pytest.param((), id="empty_tuple"),
        pytest.param({"a": 1}, id="dict_one"),
        pytest.param(PluralizeSized(), id="sized"),
        pytest.param(object(), id="object"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_pluralize_matches_django(value, autoescape):
    literals = "".join(
        "[{{ value|pluralize:'" + argument + "' }}]"
        for argument in PLURALIZE_ARGUMENTS
        if isinstance(argument, str)
    )
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ value|pluralize }}]" + literals + "{% for argument in arguments %}"
        "[{{ value|pluralize:argument }}]{% endfor %}"
        "[{{ missing|pluralize }}][{{ value|pluralize:missing }}]"
        "[{{ value|pluralize:1 }}][{{ value|pluralize|length }}]"
        "{% endautoescape %}"
    )
    context = {"value": value, "arguments": PLURALIZE_ARGUMENTS}
    django, rusty = render_both(source, context)
    assert rusty == django


def test_pluralize_literals():
    source = (
        "[{{ 1|pluralize }}][{{ 2|pluralize }}][{{ 1.0|pluralize:'y,ies' }}]"
        "[{{ 'x'|pluralize }}][{{ 0|pluralize:'a,b,c' }}]"
    )
    django, rusty = render_both(source, {})
    assert rusty == django


def test_pluralize_safe_argument():
    source = "{{ value|pluralize:suffix }}"
    context = {"value": 2, "suffix": mark_safe("<i>,<b>")}
    django, rusty = render_both(source, context)
    assert rusty == django


def test_pluralize_replaced_builtin(monkeypatch):
    monkeypatch.setitem(
        register.filters, "pluralize", lambda value, arg="s": "replacement"
    )
    django, rusty = render_both("{{ value|pluralize }}", {"value": 2})
    assert rusty == django == "replacement"


GET_DIGIT_ARGUMENTS = [-1, 0, 1, 2, 3, 5, 6, 30, "1", "2", " 1", "x", 1.5, True, None]


@pytest.mark.parametrize(
    "value",
    [
        pytest.param(0, id="zero"),
        pytest.param(12345, id="int"),
        pytest.param(10**25, id="big_int"),
        pytest.param(-123, id="negative"),
        pytest.param("12345", id="digit_string"),
        pytest.param("007", id="leading_zeros"),
        pytest.param(" 12 ", id="spaces"),
        pytest.param("1_000", id="underscore"),
        pytest.param("-45", id="negative_string"),
        pytest.param("abc", id="string"),
        pytest.param(mark_safe("123"), id="safe"),
        pytest.param(12.7, id="float"),
        pytest.param(True, id="bool"),
        pytest.param(Decimal("12"), id="decimal"),
        pytest.param([1], id="list"),
        pytest.param(None, id="none"),
    ],
)
def test_get_digit_matches_django(value):
    literals = "".join(
        "[{{ value|get_digit:" + repr(argument) + " }}]"
        for argument in GET_DIGIT_ARGUMENTS
        if isinstance(argument, (int, str)) and not isinstance(argument, bool)
    )
    source = (
        literals + "{% for argument in arguments %}"
        "[{{ value|get_digit:argument }}]{% endfor %}"
        "[{{ missing|get_digit:1 }}][{{ value|get_digit:2|add:1 }}]"
    )
    context = {"value": value, "arguments": GET_DIGIT_ARGUMENTS}
    django, rusty = render_both(source, context)
    assert rusty == django


def test_get_digit_literals():
    source = "[{{ 123|get_digit:1 }}][{{ '9876'|get_digit:3 }}][{{ 5|get_digit:9 }}]"
    django, rusty = render_both(source, {})
    assert rusty == django


def test_get_digit_replaced_builtin(monkeypatch):
    monkeypatch.setitem(register.filters, "get_digit", lambda value, arg: "replacement")
    django, rusty = render_both("{{ value|get_digit:1 }}", {"value": 123})
    assert rusty == django == "replacement"


URL_VALUES = [
    pytest.param("", id="empty"),
    pytest.param("abc-XYZ_0.9~", id="unreserved"),
    pytest.param("a b&c=d/e?f#g", id="reserved"),
    pytest.param("/I ♥ Django/ 日本 😀", id="non_ascii"),
    pytest.param("100%", id="percent"),
    pytest.param("<a href='x'>\"q\"</a>", id="html"),
    pytest.param("[]=:;$&()+,!?*@'~", id="iri_safe"),
    pytest.param(mark_safe("<b>&amp; é</b>"), id="safe"),
    pytest.param(translation.gettext_lazy("Good morning"), id="lazy"),
    pytest.param(12345, id="int"),
    pytest.param(1.5, id="float"),
    pytest.param(True, id="bool"),
    pytest.param(["a b"], id="list"),
    pytest.param(None, id="none"),
]


@pytest.mark.parametrize("value", URL_VALUES)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_urlencode_iriencode_match_django(value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ value|urlencode }}][{{ value|urlencode:'' }}][{{ value|urlencode:'/&= ' }}]"
        "[{{ value|urlencode:'é' }}][{{ value|urlencode:safe }}][{{ value|urlencode:nothing }}]"
        "[{{ value|iriencode }}][{{ value|safe|iriencode }}][{{ missing|iriencode }}]"
        "[{{ missing|urlencode }}][{{ value|iriencode|length }}]"
        "{% endautoescape %}"
    )
    django, rusty = render_both(
        source, {"value": value, "safe": mark_safe("<>"), "nothing": None}
    )
    assert rusty == django


@pytest.mark.parametrize(
    "value",
    [
        pytest.param("1-800-FLOWERS", id="letters"),
        pytest.param("abcdefghijklmnopqrstuvwxyz", id="lower"),
        pytest.param("ABCDEFGHIJKLMNOPQRSTUVWXYZ", id="upper"),
        pytest.param("<call> & 'x'", id="html"),
        pytest.param("", id="empty"),
        pytest.param("ÄÖ İ ß", id="non_ascii"),
        pytest.param(mark_safe("<b>CALL</b>"), id="safe"),
        pytest.param(translation.gettext_lazy("Good morning"), id="lazy"),
        pytest.param(12345, id="int"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_phone2numeric_matches_django(value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ value|phone2numeric }}][{{ value|lower|phone2numeric }}]"
        "[{{ missing|phone2numeric }}][{{ value|phone2numeric|length }}]"
        "{% endautoescape %}"
    )
    django, rusty = render_both(source, {"value": value})
    assert rusty == django


def test_string_filter_literals():
    source = (
        "[{{ '1-800-GO'|phone2numeric }}][{{ 'a b/c'|urlencode }}]"
        "[{{ 'a b/c'|urlencode:'' }}][{{ '/a b?é'|iriencode }}][{{ 12|urlencode }}]"
    )
    django, rusty = render_both(source, {})
    assert rusty == django


@pytest.mark.parametrize(
    "name,replacement",
    [
        ("phone2numeric", lambda value: "replacement"),
        ("urlencode", lambda value, safe=None: "replacement"),
        ("iriencode", lambda value: "replacement"),
    ],
)
def test_string_filters_replaced_builtin(monkeypatch, name, replacement):
    monkeypatch.setitem(register.filters, name, replacement)
    django, rusty = render_both("{{ value|" + name + " }}", {"value": "a b"})
    assert rusty == django == "replacement"


class SeqHtml:
    def __html__(self):
        return "<i>html</i>"

    def __str__(self):
        return "<str>"


class SeqSafeSubclass(type(mark_safe(""))):
    pass


SEQUENCE_VALUES = [
    pytest.param(["<a>", "b&c", "'d'"], id="strings"),
    pytest.param(("<a>", '"q"'), id="tuple"),
    pytest.param([mark_safe("<b>"), "<i>"], id="safe_item"),
    pytest.param([SeqSafeSubclass("<s>")], id="safe_subclass"),
    pytest.param([SeqHtml(), 1, 2.5, None, True], id="objects"),
    pytest.param([translation.gettext_lazy("Good morning"), "<x>"], id="lazy"),
    pytest.param([["<n>"], ("<t>",)], id="nested"),
    pytest.param([], id="empty"),
    pytest.param("<ab>", id="string"),
    pytest.param(mark_safe("<ab>"), id="safe_string"),
    pytest.param(range(3), id="range"),
    pytest.param({"<k>": 1}, id="dict"),
    pytest.param(12, id="int"),
    pytest.param(None, id="none"),
]


@pytest.mark.parametrize("value", SEQUENCE_VALUES)
@pytest.mark.parametrize("name", ["escapeseq", "safeseq", "make_list"])
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_sequence_filters_match_django(value, name, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "{% for item in value|NAME %}[{{ item }}]{% endfor %}"
        "[{{ value|NAME|join:', ' }}][{{ value|NAME|first }}][{{ value|NAME|length }}]"
        "[{{ value|NAME }}][{{ missing|NAME|length }}]"
        "{% endautoescape %}"
    ).replace("NAME", name)
    django, rusty = render_both(source, {"value": value})
    assert rusty == django


def test_sequence_filter_literals():
    source = (
        "[{{ 'a<b'|make_list|join:',' }}][{{ 123|make_list|length }}]"
        "[{{ 'é😀'|make_list|last }}]"
    )
    django, rusty = render_both(source, {})
    assert rusty == django


@pytest.mark.parametrize("name", ["escapeseq", "safeseq", "make_list"])
def test_sequence_filters_replaced_builtin(monkeypatch, name):
    monkeypatch.setitem(register.filters, name, lambda value: "replacement")
    django, rusty = render_both("{{ value|" + name + " }}", {"value": ["a"]})
    assert rusty == django == "replacement"
