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


@pytest.mark.parametrize(
    "value",
    [
        pytest.param(["<a>", "b", mark_safe("<c>"), 1, None], id="list"),
        pytest.param(("x", "y"), id="tuple"),
        pytest.param([], id="empty"),
        pytest.param((), id="empty_tuple"),
        pytest.param("<abc>", id="string"),
        pytest.param(mark_safe("<abc>"), id="safe_string"),
        pytest.param(range(5), id="range"),
        pytest.param({"a": 1}, id="dict"),
        pytest.param(12, id="int"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_random_matches_django(value, autoescape):
    import random

    source = (
        "{% autoescape " + autoescape + " %}"
        "{% for i in 'abcdefgh' %}[{{ value|random }}]{% endfor %}"
        "[{{ missing|random }}][{{ value|random|length }}]"
        "{% endautoescape %}"
    )
    results = []
    for name in ("django", "rusty"):
        random.seed(1234)
        try:
            results.append(engines[name].from_string(source).render({"value": value}))
        except Exception as error:
            results.append((type(error), str(error)))
    django, rusty = results
    assert rusty == django


class PprintError:
    def __repr__(self):
        raise ValueError("no repr <here>")


class PprintBaseError:
    def __repr__(self):
        raise KeyboardInterrupt


@pytest.mark.parametrize(
    "value",
    [
        pytest.param({"b": [1, 2], "a": "<x>", "c": {"d": None}}, id="dict"),
        pytest.param(list(range(40)), id="long_list"),
        pytest.param(["x" * 50, "y" * 50], id="wrapping"),
        pytest.param("<a & b>", id="string"),
        pytest.param(mark_safe("<a & b>"), id="safe_string"),
        pytest.param(translation.gettext_lazy("Good morning"), id="lazy"),
        pytest.param(12, id="int"),
        pytest.param(1.5, id="float"),
        pytest.param(True, id="bool"),
        pytest.param(Decimal("1.50"), id="decimal"),
        pytest.param(datetime.date(2026, 1, 2), id="date"),
        pytest.param(PprintError(), id="error"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_pprint_matches_django(value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ value|pprint }}][{{ value|safe|pprint }}][{{ missing|pprint }}]"
        "[{{ value|pprint|length }}]"
        "{% endautoescape %}"
    )
    django, rusty = render_both(source, {"value": value})
    assert rusty == django


def test_pprint_literals():
    source = "[{{ 'a<b'|pprint }}][{{ 12|pprint }}][{{ 1.5|pprint }}]"
    django, rusty = render_both(source, {})
    assert rusty == django


def test_pprint_base_exception_propagates():
    for name in ("django", "rusty"):
        with pytest.raises(KeyboardInterrupt):
            engines[name].from_string("{{ value|pprint }}").render(
                {"value": PprintBaseError()}
            )


@pytest.mark.parametrize("name", ["random", "pprint"])
def test_random_pprint_replaced_builtin(monkeypatch, name):
    monkeypatch.setitem(register.filters, name, lambda value: "replacement")
    django, rusty = render_both("{{ value|" + name + " }}", {"value": ["a"]})
    assert rusty == django == "replacement"


INT_ARGUMENTS = [
    "+4",
    " 4 ",
    "\t5\n",
    "\x1c3\x1f",
    "1_0",
    "1__0",
    "_1",
    "4_",
    "-0",
    "+-1",
    "1e3",
    "0x10",
    "٣",
    "\xa04",
    "99999999999999999999",
    4.9,
    -2.5,
    float("inf"),
    float("nan"),
    True,
    False,
    mark_safe("3"),
]


@pytest.mark.parametrize(
    "name", ["ljust", "rjust", "center", "truncatewords", "truncatechars", "get_digit"]
)
def test_python_int_arguments_match_django(name):
    value = "alpha beta gamma delta" if "trunc" in name else "12345"
    source = "{% for argument in arguments %}[{{ value|NAME:argument }}]{% endfor %}"
    context = {"value": value, "arguments": INT_ARGUMENTS}
    django, rusty = render_both(source.replace("NAME", name), context)
    if name == "center" and isinstance(django, tuple):
        # Rusty reports `center`'s conversion error with its own message.
        assert rusty[0] == django[0]
    else:
        assert rusty == django


@pytest.mark.parametrize("argument", INT_ARGUMENTS + ["2", "-1"])
def test_floatformat_python_int_arguments_match_django(argument):
    django, rusty = render_both(
        "{{ value|floatformat:argument }}", {"value": 1.23456, "argument": argument}
    )
    assert rusty == django


def join_generator():
    yield "<a>"
    yield mark_safe("<b>")


def join_failing_generator():
    yield "a"
    raise TypeError("stop")


class JoinNotIterable:
    pass


@pytest.mark.parametrize(
    "value",
    [
        pytest.param(range(4), id="range"),
        pytest.param("<ab>", id="string"),
        pytest.param(mark_safe("<ab>"), id="safe_string"),
        pytest.param(translation.gettext_lazy("Good"), id="lazy"),
        pytest.param({"<k>": 1, "j": 2}, id="dict"),
        pytest.param({"<k>": 1}.keys(), id="dict_keys"),
        pytest.param(frozenset(["<x>"]), id="frozenset"),
        pytest.param(12, id="int"),
        pytest.param(1.5, id="float"),
        pytest.param(JoinNotIterable(), id="not_iterable"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_join_other_values_match_django(value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ value|join:', ' }}][{{ value|join:'&' }}][{{ missing|join:',' }}]"
        "[{{ value|join:'-'|length }}]"
        "{% endautoescape %}"
    )
    django, rusty = render_both(source, {"value": value})
    assert rusty == django


@pytest.mark.parametrize("make", [join_generator, join_failing_generator])
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_join_generators_match_django(make, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}{{ value|join:'|' }}{% endautoescape %}"
    )
    results = []
    for name in ("django", "rusty"):
        try:
            results.append(engines[name].from_string(source).render({"value": make()}))
        except Exception as error:
            results.append((type(error), str(error)))
    django, rusty = results
    # A consumed generator renders with its address, so compare the kind of
    # result rather than the text when it is returned unchanged.
    if isinstance(django, str) and "generator object" in django:
        assert isinstance(rusty, str) and "generator object" in rusty
    else:
        assert rusty == django


class SliceStr:
    def __str__(self):
        return "1:3"


@pytest.mark.parametrize(
    "argument",
    [mark_safe("1:3"), mark_safe("::-1"), Decimal("2"), SliceStr(), ["x"], {"a": 1}],
    ids=["safe", "safe_reverse", "decimal", "str_object", "list", "dict"],
)
@pytest.mark.parametrize("value", ["abcdef", ["a", "b", "c", "d"]], ids=["str", "list"])
def test_slice_object_arguments_match_django(argument, value):
    django, rusty = render_both(
        "{{ value|slice:argument }}", {"value": value, "argument": argument}
    )
    assert rusty == django


@pytest.mark.parametrize(
    "value",
    [
        pytest.param(1234567, id="int"),
        pytest.param(translation.gettext_lazy("Good morning to you"), id="lazy"),
        pytest.param(Decimal("1.5"), id="decimal"),
        pytest.param(["<a>", "b c"], id="list"),
        pytest.param(mark_safe("<b>one two three</b>"), id="safe"),
        pytest.param(None, id="none"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_stringfilter_values_match_django(value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "[{{ value|truncatewords:2 }}][{{ value|ljust:12 }}][{{ value|rjust:12 }}]"
        "[{{ value|center:13 }}][{{ value|center:0 }}][{{ value|center:-3 }}]"
        "{% endautoescape %}"
    )
    django, rusty = render_both(source, {"value": value})
    assert rusty == django


@pytest.mark.parametrize("width", [-1, 0, 1, 2, 3, 4, 5, 6, 7, 10, "8", " 9"])
@pytest.mark.parametrize("value", ["", "a", "ab", "abc", "é日", mark_safe("<b>")])
def test_center_widths_match_django(value, width):
    django, rusty = render_both(
        "[{{ value|center:width }}]", {"value": value, "width": width}
    )
    assert rusty == django


ACTIVE_LANGUAGE_SOURCE = (
    "{% load i18n %}[{{ f|floatformat:2 }}][{{ s|truncatechars:5 }}]"
    "{% translate 'Good morning' %}"
)
ACTIVE_LANGUAGE_CONTEXT = {"f": 1234.5678, "s": "abcdefghij"}


def render_active_language():
    return [
        engines[name]
        .from_string(ACTIVE_LANGUAGE_SOURCE)
        .render(ACTIVE_LANGUAGE_CONTEXT)
        for name in ("django", "rusty")
    ]


def test_active_language_states_match_django(settings):
    settings.USE_THOUSAND_SEPARATOR = True
    seen = []
    for setup in [
        lambda: translation.deactivate(),
        lambda: translation.activate("de"),
        lambda: translation.activate("ckb"),
        lambda: translation.deactivate_all(),
        lambda: translation.activate("he"),
        lambda: translation.deactivate(),
    ]:
        setup()
        django, rusty = render_active_language()
        assert rusty == django
        seen.append(django)
    translation.deactivate()
    # The states render differently, so a stale cache would show.
    assert len(set(seen)) >= 4


def test_active_language_follows_language_code_setting(settings):
    translation.deactivate()
    first = render_active_language()
    settings.LANGUAGE_CODE = "de"
    second = render_active_language()
    assert first[0] == first[1]
    assert second[0] == second[1]
    assert first[0] != second[0]


def test_active_language_in_other_threads():
    import threading

    results = {}

    def render(language):
        with translation.override(language):
            results[language] = render_active_language()

    threads = [threading.Thread(target=render, args=(code,)) for code in ["de", "he"]]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    for django, rusty in results.values():
        assert rusty == django
    assert results["de"][0] != results["he"][0]


def test_active_language_in_async_tasks():
    import asyncio

    async def render(language):
        translation.activate(language)
        await asyncio.sleep(0)
        return render_active_language()

    async def main():
        return await asyncio.gather(render("de"), render("he"))

    for django, rusty in asyncio.run(main()):
        assert rusty == django


def test_active_language_with_thread_critical_local(monkeypatch):
    from asgiref.local import Local
    from django.utils.translation import trans_real

    # Not the context-variable storage, so Rusty asks Django.
    monkeypatch.setattr(trans_real, "_active", Local(thread_critical=True))
    with translation.override("de"):
        django, rusty = render_active_language()
    assert rusty == django
    assert "1234,57" in rusty


FILESIZE_VALUES = [
    0,
    1,
    2,
    5,
    1023,
    1024,
    1025,
    1280,  # 1.25 KB, a tie that rounds to even
    1331,
    1536,
    10239,
    1048575,  # rounds up to 1024.0 KB
    1048576,
    123456789,
    2**30 * 1.5,
    2**40 * 3 + 2**39,
    2**50 * 7,
    2**53 - 1,
    2**53,
    2**60,
    -1,
    -1536,
    -(2**40),
    True,
    1536.9,
    "2048",
    " 2048 ",
    "1_024",
    "x",
    "",
    None,
    Decimal("1536"),
    [1],
]


@pytest.mark.parametrize("language", ["en", "de", "fr", "he", "ckb"])
@pytest.mark.parametrize("thousand_separator", [False, True])
def test_filesizeformat_matches_django(settings, language, thousand_separator):
    settings.USE_THOUSAND_SEPARATOR = thousand_separator
    source = (
        "{% for value in values %}[{{ value|filesizeformat }}]{% endfor %}"
        "[{{ missing|filesizeformat }}][{{ safe|filesizeformat }}]"
        "[{{ 2048|filesizeformat }}][{{ '5000'|filesizeformat }}]"
    )
    context = {"values": FILESIZE_VALUES, "safe": mark_safe("4096")}
    with translation.override(language):
        django, rusty = render_both(source, context)
    assert rusty == django


def test_filesizeformat_every_tenth_matches_django():
    values = list(range(1000, 1100)) + [1024 * n // 20 for n in range(20, 21000, 7)]
    source = "{% for value in values %}[{{ value|filesizeformat }}]{% endfor %}"
    django, rusty = render_both(source, {"values": values})
    assert rusty == django


@pytest.mark.parametrize(
    "text",
    ["%s KiB", "KB: %s", "%s", "%s %% KB"],
    ids=["suffix", "prefix", "bare", "escaped_percent"],
)
def test_filesizeformat_translated_units(monkeypatch, text):
    gettext = translation._trans.gettext

    def translate(message):
        if message == "%s KB":
            return text
        return gettext(message)

    monkeypatch.setattr(translation._trans, "gettext", translate)
    source = "[{{ 2048|filesizeformat }}][{{ 1536|filesizeformat }}]"
    django, rusty = render_both(source, {})
    assert rusty == django


def test_filesizeformat_replaced_builtin(monkeypatch):
    monkeypatch.setitem(register.filters, "filesizeformat", lambda value: "replacement")
    django, rusty = render_both("{{ value|filesizeformat }}", {"value": 2048})
    assert rusty == django == "replacement"


FLOATFORMAT_STRINGS = [
    "34.23234",
    "34.00000",
    "-34.26",
    "+12.5",
    "66666.6666",
    ".5",
    "5.",
    "1e3",
    "1.5E-3",
    "-0",
    "1234567.891",
    " 1.5",
    "1_000.5",
    "NaN",
    "Infinity",
    "-inf",
    "1e",
    ".",
    "",
    "abc",
    "１２",
]
FLOATFORMAT_SUFFIXES = [
    "",
    "2",
    "-2",
    "0",
    "g",
    "u",
    "gu",
    "ug",
    "2g",
    "-3g",
    "2u",
    "-1u",
    "2gu",
    "3ug",
    " 2g",
    "xg",
    "2x",
]


@pytest.mark.parametrize("language", ["en", "de", "pl"])
@pytest.mark.parametrize("thousand_separator", [False, True])
def test_floatformat_strings_and_suffixes_match_django(
    settings, language, thousand_separator
):
    settings.USE_THOUSAND_SEPARATOR = thousand_separator
    source = (
        "{% for value in values %}{% for argument in arguments %}"
        "[{{ value|floatformat:argument }}]{% endfor %}"
        "[{{ value|floatformat }}][{{ value|floatformat:'2g' }}]{% endfor %}"
    )
    values = [*FLOATFORMAT_STRINGS, mark_safe("1234.5"), 6666.6666, 10000, True, False]
    context = {"values": values, "arguments": FLOATFORMAT_SUFFIXES}
    with translation.override(language):
        django, rusty = render_both(source, context)
    assert rusty == django


def test_floatformat_unlocalized_settings(settings):
    settings.DECIMAL_SEPARATOR = ","
    settings.THOUSAND_SEPARATOR = "'"
    settings.NUMBER_GROUPING = 3
    source = (
        "[{{ value|floatformat:'2u' }}][{{ value|floatformat:'2gu' }}]"
        "[{{ value|floatformat:'g' }}][{{ value|floatformat:'ug' }}]"
    )
    with translation.override("de"):
        django, rusty = render_both(source, {"value": 1234567.891})
    assert rusty == django
    settings.NUMBER_GROUPING = 0
    with translation.override("de"):
        django, rusty = render_both(source, {"value": 1234567.891})
    assert rusty == django


def test_floatformat_empty_argument_matches_django():
    django, rusty = render_both(
        "{{ value|floatformat:argument }}", {"value": 1.5, "argument": ""}
    )
    assert rusty[0] == django[0]


class ListItem:
    def __str__(self):
        return "<item>"


class SafeListItem:
    def __html__(self):
        return "<b>safe</b>"

    def __str__(self):
        return "<b>unsafe</b>"


class ListSubclass(list):
    pass


def list_generator():
    yield "child"


def recursive_list():
    items = ["a"]
    items.append(items)
    return items


@pytest.mark.parametrize(
    "value",
    [
        [],
        ["a"],
        ["<a>", "b&c"],
        ["a", ["b", ["c", "d"]], "e"],
        ["a", [], "b"],
        ["a", (), "b", ["c"]],
        [["a", "b"], "c"],
        ("a", ("b", "c")),
        ["a", ["b"], ["c"]],
        [mark_safe("<b>"), ["<i>", mark_safe("<u>")]],
        [1, 2.5, None, True, Decimal("1.5")],
        [ListItem(), SafeListItem()],
        [translation.gettext_lazy("Yes"), ["<x>"]],
        ["a", ListSubclass(["b"])],
        ListSubclass(["a", ["b"]]),
        "abc",
        {"a": 1},
        ["é日", ["\t"]],
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_unordered_list_matches_django(value, autoescape):
    source = (
        "{% autoescape "
        + autoescape
        + " %}{{ value|unordered_list }}{% endautoescape %}"
    )
    django, rusty = render_both(source, {"value": value})
    assert rusty == django


def test_unordered_list_generator_children_match_django():
    source = "{{ value|unordered_list }}"
    outputs = [
        engines[name].from_string(source).render({"value": ["a", list_generator()]})
        for name in ("django", "rusty")
    ]
    assert outputs[0] == outputs[1]
    assert "child" in outputs[0]


def test_unordered_list_deep_nesting_matches_django():
    value = ["leaf"]
    for _ in range(150):
        value = ["item", value]
    django, rusty = render_both("{{ value|unordered_list }}", {"value": value})
    assert rusty == django


def test_unordered_list_recursive_matches_django():
    django, rusty = render_both(
        "{{ value|unordered_list }}", {"value": recursive_list()}
    )
    assert rusty[0] == django[0]


def test_unordered_list_replaced_builtin(monkeypatch):
    monkeypatch.setitem(register.filters, "unordered_list", lambda value: "replacement")
    django, rusty = render_both("{{ value|unordered_list }}", {"value": ["a"]})
    assert rusty == django == "replacement"


STRIPTAGS_VALUES = [
    "",
    "plain text",
    "a > b",
    "a < b",
    "<b>bold</b> text",
    "<p class='x'>one</p><br/><br />two",
    '<a href="/path?a=1;b=2" title="t">link</a>',
    "<a href=/path>unquoted</a>",
    "<a title='x>y'>quoted gt</a>",
    '<a title="x<y">quoted lt</a>',
    '<a "x">stray quote</a>',
    "</a b='c'>end tag quote",
    "<b>&amp; &lt;i&gt; &#39; &amp</b>",
    "Fish &amp; chips",
    "<!-- comment -->text",
    "<!DOCTYPE html><p>x</p>",
    "<?php echo 1 ?>x",
    "<script>alert('<b>')</script>after",
    "<style>p { color: red }</style>after",
    "<textarea><b>x</b></textarea>",
    "<title>a<b>c</b></title>",
    "<SCRIPT>x</SCRIPT>y",
    "<b",
    "<b>unclosed",
    "</>",
    "< b>spaced</b>",
    "<1>digit</1>",
    "<my-tag data-x=1>custom</my-tag>",
    "<b\nclass='a'>newline</b>",
    "<b\tclass='a'>tab</b>",
    "<b/class='a'>slash</b>",
    "<<b>>double<</b>>",
    "<sc<script>ript>alert(1)</script>",
    "<a>é日</a>本",
    "<img src=x onerror=alert(1)>",
    "<b`x`>backtick</b>",
]


@pytest.mark.parametrize("value", STRIPTAGS_VALUES)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_striptags_matches_django(value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "{{ value|striptags }}|{{ safe|striptags }}"
        "{% endautoescape %}"
    )
    django, rusty = render_both(source, {"value": value, "safe": mark_safe(value)})
    assert rusty == django


@pytest.mark.parametrize(
    "value",
    [123, None, ["<b>x</b>"], translation.gettext_lazy("Yes"), ListItem()],
    ids=["int", "none", "list", "lazy", "object"],
)
def test_striptags_non_strings_match_django(value):
    django, rusty = render_both("{{ value|striptags }}", {"value": value})
    assert rusty == django


def test_striptags_random_text_matches_django():
    import random

    generator = random.Random(227)
    pieces = ["<", ">", "/", "b", "a", " ", "=", '"', "'", "&", "x", "\n", "!", "-"]
    values = [
        "".join(generator.choice(pieces) for _ in range(generator.randint(1, 16)))
        for _ in range(2000)
    ]
    source = "{% for value in values %}[{{ value|striptags }}]{% endfor %}"
    django, rusty = render_both(source, {"values": values})
    assert rusty == django


def test_striptags_long_open_tag_matches_django():
    value = "<a" + "<" * 60 + "b" * 1000
    django, rusty = render_both("{{ value|striptags }}", {"value": value})
    assert rusty == django


def test_striptags_replaced_builtin(monkeypatch):
    monkeypatch.setitem(register.filters, "striptags", lambda value: "replacement")
    django, rusty = render_both("{{ value|striptags }}", {"value": "<b>a</b>"})
    assert rusty == django == "replacement"


STRINGFORMAT_SPECS = [
    *(
        flags + width + precision + conversion
        for flags in ["", "-", "0", "+", " ", "#", "-0", "0-", "+ ", " +", "#0", "-#+"]
        for width in ["", "1", "8"]
        for precision in ["", ".0", ".2", "."]
        for conversion in "sdixXofFeE"
    ),
    "r",
    "a",
    "%",
    "5",
    "",
    "ds",
    "(key)s",
    "*d",
    "ld",
    "1000d",
    "10.30f",
]

STRINGFORMAT_VALUES = [
    0,
    7,
    -42,
    255,
    2**53,
    2**53 + 1,
    -(2**63),
    2**70,
    0.0,
    -0.0,
    3.14159,
    -2.5,
    0.5,
    1.5,
    2.675,
    1e-7,
    123456789.125,
    1e22,
    float("inf"),
    float("nan"),
    True,
    "",
    "text",
    "é日本語",
    "<b>",
    mark_safe("<i>safe</i>"),
    translation.gettext_lazy("Yes"),
    None,
    (1, 2),
    [1, 2],
    Decimal("1.5"),
]


@pytest.mark.parametrize("autoescape", ["on", "off"])
@pytest.mark.parametrize("value", STRINGFORMAT_VALUES, ids=repr)
def test_stringformat_matches_django(value, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "{{ value|stringformat:spec }}{% endautoescape %}"
    )
    templates = {
        name: engines[name].from_string(source) for name in ("django", "rusty")
    }
    mismatches = []
    rendered = 0
    for spec in STRINGFORMAT_SPECS:
        results = []
        for name in ("django", "rusty"):
            try:
                results.append(templates[name].render({"value": value, "spec": spec}))
            except Exception as error:
                results.append((type(error), str(error)))
        if results[0] != results[1]:
            mismatches.append((spec, *results))
        rendered += isinstance(results[0], str)
    assert mismatches == []
    assert rendered


@pytest.mark.parametrize("value", [65, 2**70, "a", "ab", 1.5])
def test_stringformat_character_matches_django(value):
    django, rusty = render_both("{{ value|stringformat:'c' }}", {"value": value})
    assert rusty == django


@pytest.mark.parametrize(
    "expression",
    [
        "7|stringformat:'03d'",
        "7|stringformat:'s'",
        "1.5|stringformat:'.3f'",
        "'abc'|stringformat:'5s'",
        "missing|stringformat:'s'",
        "missing|stringformat:'d'",
        "value|stringformat:5",
        "value|stringformat:missing",
        "value|stringformat",
    ],
)
def test_stringformat_literals_match_django(expression):
    source = "{{ " + expression + " }}"
    django, rusty = render_both(source, {"value": 7})
    assert type(rusty) is type(django)
    if isinstance(django, str):
        assert rusty == django


def test_stringformat_replaced_builtin(monkeypatch):
    monkeypatch.setitem(
        register.filters, "stringformat", lambda value, arg: "replacement"
    )
    django, rusty = render_both("{{ value|stringformat:'d' }}", {"value": 1})
    assert rusty == django == "replacement"


class Person:
    def __init__(self, name, age):
        self.name = name
        self.age = age

    def __repr__(self):
        return f"Person({self.name!r})"


class ItemErrors:
    def __init__(self, error, name):
        self.error = error
        self.name = name

    def __getitem__(self, key):
        raise self.error

    def __repr__(self):
        return f"ItemErrors({self.name!r})"


class Unorderable:
    def __lt__(self, other):
        raise ValueError("cannot compare")


PEOPLE = [
    {"name": "Bea", "age": 30, "info": {"city": "Oslo"}, "tags": ["b", "a"]},
    {"name": "al", "age": 25, "info": {"city": "Lima"}, "tags": ["c"]},
    {"name": "Cy", "age": 30, "info": {"city": "Ávila"}, "tags": ["a"]},
    {"name": "Dee", "age": 2.5, "info": {"city": "Oslo"}, "tags": []},
]


@pytest.mark.parametrize("name", ["dictsort", "dictsortreversed"])
@pytest.mark.parametrize(
    "value,argument",
    [
        (PEOPLE, "name"),
        (PEOPLE, "age"),
        (PEOPLE[:3], "age"),
        (PEOPLE, "info.city"),
        (PEOPLE, "tags"),
        (PEOPLE, "missing"),
        (PEOPLE, "items"),
        (PEOPLE, "name.upper"),
        (PEOPLE, "_private"),
        (PEOPLE, "info._private"),
        (PEOPLE, "0"),
        (PEOPLE, "1e3"),
        (PEOPLE, "nan"),
        (PEOPLE, " inf "),
        (PEOPLE, ""),
        (tuple(PEOPLE), "name"),
        ([], "name"),
        ([{"a": 2}, {"a": 1.5}, {"a": True}], "a"),
        ([{"a": 2**70}, {"a": -1}, {"a": 3}], "a"),
        ([{"a": 1.0}, {"a": -0.0}, {"a": 0.0}, {"a": float("nan")}], "a"),
        ([{"a": "b"}, {"a": 1}], "a"),
        ([{"a": None}, {"a": None}], "a"),
        ([{"a": (2, 1)}, {"a": (1, 2)}], "a"),
        ([{"a": "\ud800"}, {"a": "b"}], "a"),
        ([Person("b", 2), Person("a", 1)], "name"),
        ([Person("b", 2), {"name": "a"}], "name"),
        ([["x", "b"], ["y", "a"]], "1"),
        ([["x", "b"], ["y", "a"]], "a.b"),
        ([ItemErrors(KeyError, "k"), ItemErrors(IndexError, "i")], "name"),
        ([ItemErrors(ValueError, "v"), ItemErrors(TypeError, "t")], "name"),
        ([{"a": {"b": "z"}}, {"a": {"b": "y"}}], "a.b"),
        ("abc", "a"),
        ({"a": 1}, "a"),
        (None, "a"),
        ([1, 2], "real"),
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_dictsort_matches_django(name, value, argument, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "{{ value|" + name + ":argument }}{% endautoescape %}"
    )
    django, rusty = render_both(source, {"value": value, "argument": argument})
    assert rusty == django


@pytest.mark.parametrize("name", ["dictsort", "dictsortreversed"])
def test_dictsort_errors_match_django(name):
    source = "{{ value|" + name + ":'a' }}"
    for value in [
        [{"a": Unorderable()}, {"a": Unorderable()}],
        [ItemErrors(RuntimeError, "r")],
    ]:
        django, rusty = render_both(source, {"value": value})
        assert rusty == django
        assert isinstance(django, tuple)


@pytest.mark.parametrize("name", ["dictsort", "dictsortreversed"])
def test_dictsort_chains_match_django(name):
    source = (
        "{% for person in people|" + name + ":'age' %}{{ person.name }},{% endfor %}"
        "{{ people|" + name + ":'name'|first|safe }}"
        "{{ people|" + name + ":'info.city'|length }}"
    )
    django, rusty = render_both(source, {"people": PEOPLE})
    assert rusty == django


@pytest.mark.parametrize("name", ["dictsort", "dictsortreversed"])
def test_dictsort_replaced_builtin(monkeypatch, name):
    monkeypatch.setitem(register.filters, name, lambda value, arg: "replacement")
    django, rusty = render_both("{{ value|" + name + ":'a' }}", {"value": []})
    assert rusty == django == "replacement"


JSON_SCRIPT_FLOATS = [
    0.0,
    -0.0,
    1.0,
    -2.5,
    0.1,
    1 / 3,
    1e-4,
    9.999e-5,
    1e-5,
    1.5e-7,
    1e15,
    1e16,
    1.5e16,
    123456789012345678.0,
    1e22,
    1.7976931348623157e308,
    5e-324,
    # Halfway between two shortest forms; Python rounds to even.
    -777852399010546.25,
    float("nan"),
    float("inf"),
    float("-inf"),
]


def circular_list():
    items = []
    items.append(items)
    return items


@pytest.mark.parametrize(
    "value",
    [
        None,
        True,
        False,
        0,
        -7,
        2**70,
        *JSON_SCRIPT_FLOATS,
        "",
        "plain",
        "<script>&</script>",
        'quote " backslash \\ slash /',
        "\n\r\t\b\f\x00\x1f\x7f",
        "é日本\U0001f600",
        mark_safe("<b>safe</b>"),
        "\ud800",
        [],
        [1, "two", [3.5, None]],
        (1, (2, 3)),
        {},
        {"a": 1, "b": [True, {"c": "<d>"}]},
        {1: "int key", 2**70: "big key"},
        {True: "bool key"},
        {1.5: "float key"},
        {None: "none key"},
        {("a",): "tuple key"},
        {"\ud800": "surrogate key"},
        datetime.date(2026, 11, 5),
        Decimal("1.5"),
        translation.gettext_lazy("Yes"),
        ListSubclass([1]),
        {"nested": [datetime.time(1, 2)]},
        circular_list(),
    ],
    ids=repr,
)
@pytest.mark.parametrize(
    "argument",
    [
        "",
        ":'data'",
        ":''",
        ":'<id> & \"x\"'",
        ":_('<id>')",
        ":element_id",
        ":safe_id",
        ":empty",
        ":number",
        ":zero",
        ":nothing",
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_json_script_matches_django(value, argument, autoescape):
    source = (
        "{% autoescape " + autoescape + " %}"
        "{{ value|json_script" + argument + " }}{% endautoescape %}"
    )
    context = {
        "value": value,
        "element_id": "<id>",
        "safe_id": mark_safe("<id>"),
        "empty": "",
        "number": 5,
        "zero": 0,
        "nothing": None,
    }
    django, rusty = render_both(source, context)
    assert rusty == django


def test_json_script_deep_nesting_matches_django():
    value = "leaf"
    for _ in range(150):
        value = [value]
    django, rusty = render_both("{{ value|json_script }}", {"value": value})
    assert rusty == django


def test_json_script_random_floats_match_django():
    import random
    import struct

    generator = random.Random(234)
    values = [struct.unpack("<d", generator.randbytes(8))[0] for _ in range(3000)] + [
        generator.uniform(-1e6, 1e6) for _ in range(1000)
    ]
    django, rusty = render_both("{{ value|json_script }}", {"value": values})
    assert rusty == django


def test_json_script_replaced_builtin(monkeypatch):
    monkeypatch.setitem(
        register.filters, "json_script", lambda value, arg=None: "replacement"
    )
    django, rusty = render_both("{{ value|json_script }}", {"value": [1]})
    assert rusty == django == "replacement"


def zone_values():
    from zoneinfo import ZoneInfo

    new_york = ZoneInfo("America/New_York")
    utc = datetime.timezone.utc
    return [
        datetime.datetime(2026, 7, 4, 15, 30, 5, 123, tzinfo=utc),
        datetime.datetime(
            2026,
            1,
            2,
            3,
            4,
            5,
            tzinfo=datetime.timezone(datetime.timedelta(hours=5, minutes=30)),
        ),
        datetime.datetime(
            2026,
            1,
            2,
            3,
            4,
            5,
            tzinfo=datetime.timezone(
                -datetime.timedelta(hours=3, minutes=15), "Custom"
            ),
        ),
        datetime.datetime(
            2026,
            1,
            2,
            3,
            4,
            5,
            tzinfo=datetime.timezone(datetime.timedelta(hours=1, seconds=15)),
        ),
        datetime.datetime(
            2026,
            1,
            2,
            3,
            4,
            5,
            tzinfo=datetime.timezone(datetime.timedelta(microseconds=-1)),
        ),
        datetime.datetime(2026, 7, 4, 15, 30, tzinfo=new_york),
        datetime.datetime(2026, 12, 25, 8, 0, tzinfo=new_york),
        # Ambiguous, then imaginary, in New York.
        datetime.datetime(2026, 11, 1, 1, 30, tzinfo=new_york),
        datetime.datetime(2026, 11, 1, 1, 30, fold=1, tzinfo=new_york),
        datetime.datetime(2026, 3, 8, 2, 30, tzinfo=new_york),
        datetime.datetime(2026, 6, 1, 12, 0, tzinfo=ZoneInfo("Europe/London")),
        datetime.datetime(2026, 6, 1, 12, 0, tzinfo=ZoneInfo("Asia/Kolkata")),
        datetime.datetime(1900, 6, 1, 12, 0, tzinfo=ZoneInfo("Europe/Amsterdam")),
        datetime.datetime(2026, 7, 4, 15, 30, 5),
        datetime.datetime(2026, 11, 1, 1, 30),
        datetime.datetime(2026, 11, 1, 1, 30, fold=1),
        datetime.datetime(2026, 3, 8, 2, 30),
        datetime.datetime(1969, 12, 31, 23, 59, 59, 500000),
        datetime.date(2026, 7, 4),
        datetime.date(2026, 3, 8),
        datetime.time(15, 30),
        datetime.time(15, 30, tzinfo=datetime.timezone.utc),
    ]


ZONE_FORMATS = ["e", "I", "O", "r", "T", "U", "Z", "c O", r"\O O", "D, d M Y H:i:s O T"]


@pytest.mark.parametrize("use_tz", [False, True])
@pytest.mark.parametrize(
    "time_zone", ["America/New_York", "UTC", "Asia/Kolkata", "Australia/Lord_Howe"]
)
def test_date_timezone_specifiers_match_django(settings, use_tz, time_zone):
    settings.USE_TZ = use_tz
    settings.TIME_ZONE = time_zone
    mismatches = []
    for value in zone_values():
        for format in ZONE_FORMATS:
            for name in ("date", "time"):
                source = "{{ value|" + name + ":format }}"
                context = {"value": value, "format": format}
                django, rusty = render_both(source, context)
                # Error messages differ intentionally; compare their types.
                if isinstance(django, tuple) and isinstance(rusty, tuple):
                    django, rusty = django[0], rusty[0]
                if rusty != django:
                    mismatches.append((value, name, format, django, rusty))
    assert mismatches == []


@pytest.mark.parametrize("use_tz", [False, True])
def test_now_timezone_specifiers_match_django(settings, use_tz):
    settings.USE_TZ = use_tz
    settings.TIME_ZONE = "America/New_York"
    django, rusty = render_both('{% now "e|I|O|T|Z" %}', {})
    assert rusty == django


class FixedTzinfo(datetime.tzinfo):
    """A plain tzinfo subclass, as dateutil's are."""

    def __init__(self, hours):
        self.offset = datetime.timedelta(hours=hours)

    def utcoffset(self, dt):
        return self.offset

    def dst(self, dt):
        return datetime.timedelta(0)

    def tzname(self, dt):
        return "Fixed"


class NoOffsetTzinfo(datetime.tzinfo):
    """A tzinfo whose values count as naive."""

    def utcoffset(self, dt):
        return None

    def dst(self, dt):
        return None

    def tzname(self, dt):
        return None


class SwitchingTzinfo(FixedTzinfo):
    """Converts into another instance, as pytz does across DST changes."""

    def fromutc(self, dt):
        other = SwitchingTzinfo(self.offset.total_seconds() / 3600)
        return (dt + self.offset).replace(tzinfo=other)


class RaisingTzinfo(FixedTzinfo):
    def __init__(self, error):
        super().__init__(0)
        self.error = error

    def utcoffset(self, dt):
        raise self.error("bad offset")


def tzinfo_cases():
    plus_two = FixedTzinfo(2)
    no_offset = NoOffsetTzinfo()
    switching = SwitchingTzinfo(-5)
    utc = datetime.timezone.utc
    when = datetime.datetime(2026, 1, 15, 10, 30)
    later = datetime.datetime(2027, 3, 1, 12, 45)
    return [
        (when.replace(tzinfo=plus_two), later.replace(tzinfo=plus_two)),
        (when.replace(tzinfo=plus_two), later.replace(tzinfo=FixedTzinfo(-7))),
        (when.replace(tzinfo=plus_two), later.replace(tzinfo=utc)),
        (when.replace(tzinfo=utc), later.replace(tzinfo=plus_two)),
        (when.replace(tzinfo=plus_two), later),
        (when, later.replace(tzinfo=plus_two)),
        (when.replace(tzinfo=no_offset), later),
        (when.replace(tzinfo=no_offset), later.replace(tzinfo=NoOffsetTzinfo())),
        (when.replace(tzinfo=no_offset), later.replace(tzinfo=plus_two)),
        (when.replace(tzinfo=switching), later.replace(tzinfo=switching)),
        (when.replace(tzinfo=switching), later.replace(tzinfo=utc)),
        (when.replace(tzinfo=RaisingTzinfo(ValueError)), later),
        (when.replace(tzinfo=RaisingTzinfo(TypeError)), later),
        (later.replace(tzinfo=plus_two), when.replace(tzinfo=plus_two)),
        (datetime.date(2026, 1, 15), later.replace(tzinfo=plus_two)),
    ]


@pytest.mark.parametrize("name", ["timesince", "timeuntil"])
def test_timesince_other_tzinfo_match_django(name):
    source = "{{ value|" + name + ":other }}|{{ other|" + name + ":value }}"
    for value, other in tzinfo_cases():
        django, rusty = render_both(source, {"value": value, "other": other})
        assert rusty == django, (value, other)


@pytest.mark.parametrize("name", ["timesince", "timeuntil"])
def test_timesince_other_tzinfo_errors_match_django(name):
    when = datetime.datetime(2026, 1, 15, tzinfo=RaisingTzinfo(RuntimeError))
    django, rusty = render_both(
        "{{ value|" + name + ":other }}", {"value": when, "other": when}
    )
    assert type(rusty) is type(django)


@pytest.mark.parametrize("name", ["timesince", "timeuntil"])
@pytest.mark.parametrize("days", [-400, -3, 3, 400])
def test_timesince_other_tzinfo_now_match_django(name, days):
    delta = datetime.timedelta(days=days, minutes=1)
    for tzinfo in [FixedTzinfo(2), NoOffsetTzinfo(), SwitchingTzinfo(1)]:
        if tzinfo.utcoffset(None) is None:
            value = datetime.datetime.now().replace(tzinfo=tzinfo) + delta
        else:
            value = datetime.datetime.now(tzinfo) + delta
        django, rusty = render_both("{{ value|" + name + " }}", {"value": value})
        assert rusty == django


@pytest.mark.parametrize("name", ["timesince", "timeuntil"])
def test_timesince_pytz_and_dateutil_match_django(name):
    pytz = pytest.importorskip("pytz")
    dateutil_tz = pytest.importorskip("dateutil.tz")
    new_york = pytz.timezone("America/New_York")
    values = [
        new_york.localize(datetime.datetime(2026, 1, 15, 10, 30)),
        new_york.localize(datetime.datetime(2026, 7, 15, 10, 30)),
        datetime.datetime(
            2026, 1, 15, 10, 30, tzinfo=dateutil_tz.gettz("Europe/Paris")
        ),
        datetime.datetime(
            2026, 7, 15, 10, 30, tzinfo=dateutil_tz.gettz("Europe/Paris")
        ),
        datetime.datetime(2026, 7, 15, 10, 30, tzinfo=datetime.timezone.utc),
    ]
    source = "{{ value|" + name + ":other }}"
    for value in values:
        for other in values:
            django, rusty = render_both(source, {"value": value, "other": other})
            assert rusty == django, (value, other)
