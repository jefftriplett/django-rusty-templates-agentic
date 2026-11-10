"""`title`, `slugify`, `wordwrap`, `last` and float literals match Django.

The listed cases come from an audit that found differences. The seeded
random cases compare both engines on text built from the characters these
filters treat specially.
"""

import random

import pytest
from django.template import Context, Engine as DjangoEngine, TemplateSyntaxError
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine

ENGINES = [DjangoEngine, Engine]


def render_both(source, context):
    return [
        engine_class().from_string(source).render(Context(context))
        for engine_class in ENGINES
    ]


TITLE = [
    "ab 'cd'",
    "it's a 'quoted' word",
    "ab'c'd e'F",
    "o'neil's 1st 2ND a1b2c",
    "x'Y'z 'a 'B",
    "hello-world foo_bar baz.qux",
    "straße ß",
    "ǆungla ﬁsh ŉ",
    "ΌΣΟΣ όσος",
    "İstanbul ı",
    "٣abc ３ｄ 3D",
    "é'É à'b",
    "",
    "'",
    "a",
]


@pytest.mark.parametrize("value", TITLE)
def test_title(value):
    django_output, rusty_output = render_both("{{ value|title }}", {"value": value})
    assert rusty_output == django_output


SLUGIFY = [
    "_a",
    "-_a b_-",
    "__a__b__",
    "_-_",
    " - a - ",
    "a\x1cb\x1fc",
    "a\tb\nc\x0bd\x0ce",
    "Héllo Wörld ß ﬁ",
    "a--b__c  d",
    "--",
    "",
]


@pytest.mark.parametrize("value", SLUGIFY)
def test_slugify(value):
    django_output, rusty_output = render_both("{{ value|slugify }}", {"value": value})
    assert rusty_output == django_output


WORDWRAP = [
    "éé éé",
    "ab   cd ef",
    "  leading spaces here",
    "trailing spaces   ",
    "a\tb\tc d e f",
    "one\n\ntwo three\nfour ",
    "   ",
    "\n",
    "a\n",
    "word\r\nword word",
    "averyveryverylongword ab cd",
    "ab averyveryverylongword cd",
    "日本語 テキスト です",
    "a b c d e f g h i j",
    "x\x0cy z\x1cw",
    "",
]


@pytest.mark.parametrize("width", [1, 2, 5, 8, 14])
@pytest.mark.parametrize("value", WORDWRAP)
def test_wordwrap(value, width):
    django_output, rusty_output = render_both(
        "{% autoescape off %}{{ value|wordwrap:width }}{% endautoescape %}",
        {"value": value, "width": width},
    )
    assert rusty_output == django_output


FUZZ = {
    "title": "ab cD'e1-_ .ß\n",
    "slugify": "aB1-_ .é\t\n",
    "wordwrap:4": "ab cde é\t\n",
    "wordwrap:9": "ab cde é\t\n",
}


@pytest.mark.parametrize("name", FUZZ)
def test_random_text(name):
    alphabet = FUZZ[name]
    numbers = random.Random(366)
    source = "{% autoescape off %}{{ value|" + name + " }}{% endautoescape %}"
    templates = [engine_class().from_string(source) for engine_class in ENGINES]
    for _ in range(400):
        length = numbers.randint(0, 24)
        value = "".join(numbers.choice(alphabet) for _ in range(length))
        django_output, rusty_output = (
            template.render(Context({"value": value})) for template in templates
        )
        assert rusty_output == django_output, repr(value)


@pytest.mark.parametrize(
    "value",
    [mark_safe("a>"), "a>", [mark_safe("<b>"), "<i>"], mark_safe(""), ("<x>",)],
)
@pytest.mark.parametrize("name", ["last", "first"])
def test_last_keeps_safe_text_safe(name, value):
    django_output, rusty_output = render_both(
        "{{ value|" + name + " }}", {"value": value}
    )
    assert rusty_output == django_output


FLOAT_FILTERS = [
    "lower",
    "upper",
    "capfirst",
    "title",
    "escape",
    "force_escape",
    "addslashes",
    "escapejs",
    "cut:'x'",
    "center:9",
    "ljust:9",
    "wordwrap:9",
    "safe",
    "slugify",
    "length",
    "truncatechars:9",
    "urlencode",
    "linebreaksbr",
    "striptags",
    "default:'d'",
    "stringformat:'s'",
]


@pytest.mark.parametrize("name", [None, *FLOAT_FILTERS])
@pytest.mark.parametrize(
    "literal",
    ["1.0", "1.5", "-5", "5", "1e20", "1e-7", "0.1", "-0.0", "100.0", "1e16", "2.50"],
)
def test_number_literals(literal, name):
    source = "{{ " + literal + ("|" + name if name else "") + " }}"
    try:
        django_output = DjangoEngine().from_string(source).render(Context({}))
    except TemplateSyntaxError:
        # Before Django 6.0 a signed exponent isn't a number.
        pytest.skip("Django rejects this literal")
    assert Engine().from_string(source).render(Context({})) == django_output
