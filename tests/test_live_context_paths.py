"""A template with a custom compiled tag renders on a live Django `Context`.
Lookups, loop bindings and output read it natively where Django's methods
are unchanged, and go through Python otherwise (#371)."""

import datetime
import sys
from decimal import Decimal
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node
from django.test import override_settings
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine

MODULE = "rusty_live_context_tests"


class SetNode(Node):
    def __init__(self, name, value):
        self.name = name
        self.value = value

    def render(self, context):
        context[self.name] = self.value
        return ""


class ShowNode(Node):
    def __init__(self, names):
        self.names = names

    def render(self, context):
        values = []
        for name in self.names:
            try:
                values.append(repr(context[name]))
            except KeyError:
                values.append("-")
        return "[" + ",".join(values) + "]"


class NoescapeNode(Node):
    def render(self, context):
        context.autoescape = False
        return ""


class FlagsNode(Node):
    def __init__(self, use_l10n, use_tz):
        self.use_l10n = use_l10n
        self.use_tz = use_tz

    def render(self, context):
        context.use_l10n = self.use_l10n
        context.use_tz = self.use_tz
        return ""


FLAGS = {"none": None, "on": True, "off": False}


@pytest.fixture(autouse=True)
def library(monkeypatch):
    module = ModuleType(MODULE)
    module.register = Library()

    def set_tag(parser, token):
        _, name, value = token.split_contents()
        return SetNode(name, value)

    def show(parser, token):
        return ShowNode(token.split_contents()[1:])

    def noescape(parser, token):
        return NoescapeNode()

    def flags(parser, token):
        _, use_l10n, use_tz = token.split_contents()
        return FlagsNode(FLAGS[use_l10n], FLAGS[use_tz])

    for name, function in [
        ("set", set_tag),
        ("show", show),
        ("noescape", noescape),
        ("flags", flags),
    ]:
        function.rusty_isolated_compiler = True
        module.register.tag(name, function)
    monkeypatch.setitem(sys.modules, MODULE, module)


LIBRARIES = {"live": MODULE}


def check(source, values, context_class=Context, **options):
    options.setdefault("libraries", LIBRARIES)
    expected = (
        DjangoEngine(**options).from_string(source).render(context_class(values()))
    )
    rusty = Engine(**options).from_string(source)
    assert rusty.render(context_class(values())) == expected
    if context_class is Context:
        assert rusty.render(values()) == expected
    return expected


class Html:
    def __html__(self):
        return "<i>html</i>"

    def __str__(self):
        return "<i>str</i>"


class SafeStr:
    def __str__(self):
        return mark_safe("<b>safe</b>")


def values():
    return {
        "rows": [{"name": "<a>", "n": 1}, {"name": "b", "n": 1234}],
        "none": None,
        "obj": Html(),
        "safe": SafeStr(),
        "lst": [1, "<x>"],
        "dct": {"k": "<v>"},
        "number": 1234567,
        "price": Decimal("1234.50"),
        "ratio": 1234.5,
        "day": datetime.date(2026, 11, 11),
        "text": "<t>",
    }


OUTPUT = (
    "{{ none }}|{{ obj }}|{{ safe }}|{{ lst }}|{{ dct }}|{{ number }}|"
    "{{ price }}|{{ ratio }}|{{ day }}|{{ text }}|{{ missing }}"
)


@pytest.mark.parametrize(
    "source",
    [
        "{% load live %}{% show text %}" + OUTPUT,
        "{% load live %}{% noescape %}" + OUTPUT,
        "{% load live %}{% autoescape off %}{% show text %}"
        + OUTPUT
        + "{% endautoescape %}",
        "{% load live l10n %}{% show text %}{% localize on %}"
        + OUTPUT
        + "{% endlocalize %}",
        "{% load live l10n %}{% show text %}{% localize off %}"
        + OUTPUT
        + "{% endlocalize %}",
    ],
)
@pytest.mark.parametrize("separator", [False, True])
def test_live_output_matches_django(source, separator):
    with override_settings(USE_THOUSAND_SEPARATOR=separator):
        check(
            source, values, libraries={**LIBRARIES, "l10n": "django.templatetags.l10n"}
        )


@pytest.mark.parametrize(
    "source",
    [
        "{% load live %}{% for row in rows %}{% show row forloop %}"
        "{% for c in row.name %}{% show c %}{{ forloop.parentloop.counter }}"
        "{% endfor %}{% endfor %}{% show row c forloop %}",
        "{% load live %}{% set x one %}{{ x }}{% for row in rows %}{% set x two %}"
        "{{ x }}{% endfor %}{{ x }}{% show x %}",
        "{% load live %}{% with x=text %}{% set y inner %}{{ x }}{{ y }}{% endwith %}"
        "{{ y }}{% show x y %}",
        "{% load live %}{% for a, b in pairs %}{% show a b %}{% endfor %}",
    ],
)
@pytest.mark.parametrize("debug", [False, True])
def test_live_bindings_match_django(source, debug):
    def pairs():
        return {**values(), "pairs": [(1, 2), (3, 4)]}

    check(source, pairs, debug=debug)


class Layer(dict):
    def __contains__(self, key):
        return key == "virtual" or super().__contains__(key)

    def __getitem__(self, key):
        if key == "virtual":
            return "virtual value"
        return super().__getitem__(key)


class OverridingContext(Context):
    def __getitem__(self, key):
        if key == "text":
            return "overridden"
        return super().__getitem__(key)

    def __setitem__(self, key, value):
        super().__setitem__(key, f"set:{value}" if key == "x" else value)


def test_custom_layer_matches_django():
    def layered():
        context = Context(values())
        context.dicts.append(Layer(extra="layer"))
        return context

    source = "{% load live %}{% show text %}{{ virtual }}|{{ extra }}|{{ text }}"
    expected = DjangoEngine(libraries=LIBRARIES).from_string(source).render(layered())
    assert Engine(libraries=LIBRARIES).from_string(source).render(layered()) == expected
    assert "virtual value" in expected


@pytest.mark.parametrize(
    "source",
    [
        "{% load live %}{% show text %}{{ text }}|{{ number }}",
        "{% load live %}{% set x one %}{{ x }}{% for row in rows %}{{ row.n }}"
        "{% endfor %}{% show x %}",
    ],
)
def test_overridden_context_methods_match_django(source):
    check(source, values, OverridingContext)


def aware_values():
    return {
        **values(),
        "moment": datetime.datetime(2026, 11, 11, 23, 30, tzinfo=datetime.UTC),
        "naive": datetime.datetime(2026, 11, 11, 23, 30),
    }


@pytest.mark.parametrize("use_l10n", ["none", "on", "off"])
@pytest.mark.parametrize("use_tz", ["none", "on", "off"])
@pytest.mark.parametrize("separator", [False, True])
def test_flags_set_by_a_tag_match_django(use_l10n, use_tz, separator):
    source = (
        "{% load live %}{{ number }}|{{ moment }}|"
        f"{{% flags {use_l10n} {use_tz} %}}"
        "{{ number }}|{{ price }}|{{ ratio }}|{{ day }}|{{ moment }}|{{ naive }}"
    )
    with override_settings(
        USE_THOUSAND_SEPARATOR=separator, USE_TZ=True, TIME_ZONE="America/Chicago"
    ):
        check(source, aware_values)
