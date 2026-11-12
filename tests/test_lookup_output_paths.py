"""Native fast paths for lookups and output (#369) render what Django does."""

import datetime
from decimal import Decimal

import pytest
from django.template import Context, Engine as DjangoEngine
from django.template.library import Library
from django.utils.functional import SimpleLazyObject
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine

register = Library()


@register.simple_tag
def echo(value):
    return value


LIBRARIES = {"echo": "tests.test_lookup_output_paths"}


def check(source, context, **options):
    options.setdefault("libraries", LIBRARIES)
    try:
        expected = (
            DjangoEngine(**options).from_string(source).render(Context(dict(context)))
        )
    except Exception as error:
        with pytest.raises(type(error)):
            Engine(**options).from_string(source).render(dict(context))
        return None
    assert Engine(**options).from_string(source).render(dict(context)) == expected
    return expected


class Plain:
    attr = "class attr"

    def __init__(self):
        self.value = "instance"

    def __str__(self):
        return "<plain & str>"


class Slotted:
    __slots__ = ("value",)

    def __init__(self):
        self.value = "slot"


class CustomDir:
    def __getattr__(self, name):
        if name == "dynamic":
            return "dynamic value"
        raise AttributeError(name)

    def __dir__(self):
        return ["dynamic", "listed"]


class RaisingProperty:
    @property
    def broken(self):
        raise AttributeError("inner")

    def __getitem__(self, key):
        raise KeyError(key)


class SafeStr:
    def __str__(self):
        return mark_safe("<b>safe</b>")


class Html:
    def __html__(self):
        return "<i>html</i>"

    def __str__(self):
        return "<i>str</i>"


class LyingClass:
    @property
    def __class__(self):
        return dict


def lookups():
    return {
        "lst": ["a", "<b>", "c"],
        "tup": ("x", "y"),
        "sub": type("Sub", (list,), {"__getitem__": lambda self, i: "sub item"})(
            [1, 2]
        ),
        "plain": Plain(),
        "slotted": Slotted(),
        "custom": CustomDir(),
        "raising": RaisingProperty(),
        "lazy": SimpleLazyObject(Plain),
        "lying": LyingClass(),
        "d": {"key": "value"},
    }


@pytest.mark.parametrize(
    "source",
    [
        "{{ lst.0 }}|{{ lst.1 }}|{{ lst.2 }}|{{ lst.3 }}|{{ lst.-1 }}",
        "{{ tup.0 }}|{{ tup.1 }}|{{ tup.2 }}",
        "{{ sub.0 }}|{{ sub.5 }}",
        "{{ plain.value }}|{{ plain.attr }}|{{ plain.missing }}|{{ plain.value.upper }}",
        "{% if plain.missing %}yes{% else %}no{% endif %}",
        "{{ slotted.value }}|{{ slotted.missing }}",
        "{{ custom.dynamic }}|{{ custom.listed }}|{{ custom.other }}",
        "{{ raising.broken }}|{{ raising.missing }}",
        "{{ lazy.value }}|{{ lazy.missing }}",
        "{{ lying.missing }}|{{ lying.items }}",
        "{{ d.key }}|{{ d.missing }}|{{ d.items }}",
    ],
)
@pytest.mark.parametrize("debug", [False, True])
def test_lookups_match_django(source, debug):
    check(source, lookups(), debug=debug)


def outputs():
    return {
        "none": None,
        "lst": [1, "<a>", None],
        "dct": {"a": "<b>"},
        "obj": Plain(),
        "safe_str": SafeStr(),
        "html": Html(),
        "lazy": SimpleLazyObject(Plain),
        "number": 1234567,
        "float": 1234.5,
        "decimal": Decimal("1234.50"),
        "date": datetime.date(2026, 11, 11),
        "time": datetime.time(13, 30),
        "boolean": True,
    }


OUTPUT = (
    "{{ none }}|{{ lst }}|{{ dct }}|{{ obj }}|{{ safe_str }}|{{ html }}|"
    "{{ lazy }}|{{ number }}|{{ float }}|{{ decimal }}|{{ date }}|{{ time }}|"
    "{{ boolean }}"
)


@pytest.mark.parametrize(
    "source",
    [
        OUTPUT,
        "{% autoescape off %}" + OUTPUT + "{% endautoescape %}",
        "{% load l10n %}{% localize on %}" + OUTPUT + "{% endlocalize %}",
        "{% load l10n %}{% localize off %}" + OUTPUT + "{% endlocalize %}",
    ],
)
def test_output_matches_django(source):
    from django.test import override_settings

    check(source, outputs(), libraries={"l10n": "django.templatetags.l10n"})
    with override_settings(USE_THOUSAND_SEPARATOR=True):
        check(source, outputs(), libraries={"l10n": "django.templatetags.l10n"})


class StrSubclass(str):
    def __html__(self):
        return "html of subclass"


@pytest.mark.parametrize(
    "value",
    [
        "<plain & 'str'>",
        mark_safe("<safe>"),
        StrSubclass("<sub>"),
        Html(),
        5,
        None,
    ],
)
@pytest.mark.parametrize("autoescape", [True, False])
def test_simple_tag_escaping_matches_django(value, autoescape):
    source = "{% load echo %}{% echo value %}"
    if not autoescape:
        source = "{% autoescape off %}" + source + "{% endautoescape %}"
    check(source, {"value": value})


@pytest.mark.parametrize("value", ["<a & 'b'>", mark_safe("<s>"), Html(), 5, None])
def test_escape_filter_matches_django(value):
    check("{{ value|escape }}", {"value": value})


@pytest.mark.parametrize(
    ("left", "right"),
    [
        ("1", "2"),
        (" 1 ", "2"),
        ("1_000", "1"),
        ("١٢", "1"),
        ("ab", "cd"),
        ("ab", 1),
        (1, "ab"),
        (mark_safe("3"), "4"),
        (mark_safe("x"), mark_safe("y")),
        ([1], [2]),
        ("x", [1]),
        (StrSubclass("7"), 1),
        (1.5, "2"),
    ],
)
def test_add_matches_django(left, right):
    check("{{ left|add:right }}", {"left": left, "right": right})


@pytest.mark.parametrize(
    "source",
    [
        '{{ " 1"|add:"2" }}',
        '{{ "1 "|add:" 2 " }}',
        '{{ "1_0"|add:"1" }}',
        '{{ "x"|add:"1" }}',
        '{{ " 4"|divisibleby:"2" }}',
    ],
)
def test_add_literals_match_django(source):
    check(source, {})


@pytest.mark.parametrize(
    "values",
    [
        (175, 200, 100),
        (1, 3, 100),
        (1, 8, 4),
        (3, 8, 4),
        (5, 8, 4),
        (2.5, 1, 1),
        (3.5, 1, 1),
        (-2.5, 1, 1),
        (0, 0, 100),
        (1, 0.0, 100),
        (1e308, 1e-308, 100),
        (float("nan"), 1, 100),
        (float("inf"), 1, 100),
        (2**60, 3, 7),
        (1, 3, 2**60),
        ("50", "100", 10),
        (True, 2, 10),
        (Decimal("1.5"), 1, 1),
        (1, 2, 3.7),
        (1, 2, "x"),
    ],
)
def test_widthratio_matches_django(values):
    value, maximum, width = values
    context = {"value": value, "maximum": maximum, "width": width}
    try:
        expected = (
            DjangoEngine()
            .from_string("{% widthratio value maximum width %}")
            .render(Context(context))
        )
    except Exception as error:
        with pytest.raises(type(error)):
            Engine().from_string("{% widthratio value maximum width %}").render(context)
        return
    assert (
        Engine().from_string("{% widthratio value maximum width %}").render(context)
        == expected
    )


@pytest.mark.parametrize(
    "body",
    [
        "<p>\n  <a>x</a>\n</p>",
        "  \n<p> </p>\t ",
        "a> <b",
        "> > <",
        "><",
        "<p> <b> <i>\x1c<u>​<s>",
        "\x1f text \x1c",
        "no tags at all",
        "<p>  text  </p>",
        "",
    ],
)
def test_spaceless_matches_django(body):
    check("{% spaceless %}{{ body }}{% endspaceless %}", {"body": mark_safe(body)})
