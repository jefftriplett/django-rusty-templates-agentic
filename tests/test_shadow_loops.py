"""A template with a custom compiled tag renders on a live Django `Context`,
but a `{% for %}` whose body can't hand that context to Python renders on
native scopes, reading names it didn't bind from the live context (#371).

Each case renders with both engines, from a dict and from a `Context`."""

import sys
import weakref
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node

from django_rusty_templates import Engine

MODULE = "rusty_shadow_loop_tests"
LIBRARIES = {"shadow": MODULE}


class SetNode(Node):
    def __init__(self, name, value):
        self.name = name
        self.value = value

    def render(self, context):
        context[self.name] = self.value
        return ""


class GlobalNode(SetNode):
    def render(self, context):
        context.dicts[0][self.name] = self.value
        return ""


class StashNode(Node):
    """Keep the context reachable from a value, as Django allows."""

    def render(self, context):
        context["peek"] = lambda: context.get("row", "-")
        return ""


class WeakStashNode(Node):
    def render(self, context):
        ref = weakref.ref(context)
        context["weak"] = lambda: ref().get("row", "-")
        return ""


class BumpNode(Node):
    def render(self, context):
        context["forloop"]["counter"] = 99
        return ""


class NoescapeNode(Node):
    def render(self, context):
        context.autoescape = False
        return ""


class MarkNode(Node):
    def render(self, context):
        return "|"


NODES = {
    "set": lambda bits: SetNode(*bits),
    "global": lambda bits: GlobalNode(*bits),
    "stash": lambda bits: StashNode(),
    "weakstash": lambda bits: WeakStashNode(),
    "bump": lambda bits: BumpNode(),
    "noescape": lambda bits: NoescapeNode(),
    "mark": lambda bits: MarkNode(),
}


@pytest.fixture(autouse=True)
def library(monkeypatch):
    module = ModuleType(MODULE)
    module.register = Library()
    for name, node in NODES.items():

        def compile_function(parser, token, node=node):
            return node(token.split_contents()[1:])

        compile_function.rusty_isolated_compiler = True
        module.register.tag(name, compile_function)
    monkeypatch.setitem(sys.modules, MODULE, module)


def values():
    return {
        "rows": [{"name": "<a>", "tags": ["x", "y"]}, {"name": "b", "tags": []}],
        "pairs": [(1, "one"), (2, "two")],
        "row": "outer",
        "text": "<t>",
    }


def check(source, **options):
    options.setdefault("libraries", LIBRARIES)
    source = "{% load shadow %}" + source
    expected = DjangoEngine(**options).from_string(source).render(Context(values()))
    rusty = Engine(**options).from_string(source)
    assert rusty.render(values()) == expected
    assert rusty.render(Context(values())) == expected
    return expected


@pytest.mark.parametrize(
    "source",
    [
        # Loops after a tag, with and without loop variables of their own.
        "{% mark %}{% for row in rows %}{{ row.name }}{{ text }}{% endfor %}{{ row }}",
        "{% mark %}{% for a, b in pairs %}{{ a }}={{ b }};{% endfor %}",
        "{% mark %}{% for row in missing %}x{% empty %}{{ row }}{% endfor %}",
        "{% mark %}{% for row in rows %}{% for t in row.tags %}{{ t }}"
        "{% empty %}none{% endfor %}{% endfor %}",
        "{% mark %}{% for row in rows %}{% with n=row.name %}{{ n }}{% endwith %}"
        "{{ n }}{% endfor %}",
        "{% mark %}{% for row in rows %}{% if row.tags %}{{ row.tags|join:',' }}"
        "{% else %}-{% endif %}{% endfor %}",
        "{% mark %}{% for row in rows reversed %}{{ forloop.counter }}"
        "{{ forloop.revcounter0 }}{{ forloop.first }}{{ forloop.last }}{% endfor %}",
        # A tag inside the outer loop; the inner loop renders natively.
        "{% for row in rows %}{% mark %}{% for t in row.tags %}"
        "{{ forloop.parentloop.counter }}.{{ forloop.counter }}{{ t }}"
        "{% endfor %}{% endfor %}",
        "{% for row in rows %}{% mark %}{% for t in row.tags %}{{ forloop }}"
        "{% endfor %}{% endfor %}",
        "{% for row in rows %}{% mark %}{% for t in row.tags %}"
        "{{ forloop.parentloop.parentloop }}{% endfor %}{% endfor %}",
        # Names a tag binds, at the top or in a lower layer, are visible.
        "{% set extra E %}{% for row in rows %}{{ extra }}{% endfor %}",
        "{% for row in rows %}{% global seen yes %}{% for t in row.tags %}"
        "{{ seen }}{% endfor %}{% endfor %}{{ seen }}",
        # A tag rebinds the outer loop variable; the inner loop sees it.
        "{% for row in rows %}{% set row changed %}{% for t in 'ab' %}{{ row }}"
        "{% endfor %}{% endfor %}{{ row }}",
        # The inner loop's parent is the live forloop, changed by a tag.
        "{% for row in rows %}{% bump %}{% for t in 'ab' %}"
        "{{ forloop.parentloop.counter }}{% endfor %}{% endfor %}",
        # Flags a tag changes apply to the loop.
        "{% noescape %}{% for row in rows %}{{ row.name }}{{ text }}{% endfor %}",
        "{% autoescape off %}{% mark %}{% for row in rows %}{{ row.name }}"
        "{% endfor %}{% endautoescape %}",
        "{% mark %}{% for row in rows %}{% autoescape off %}{{ row.name }}"
        "{% endautoescape %}{{ row.name }}{% endfor %}",
        # Values that keep the context reachable see the loop's layers.
        "{% stash %}{% for row in rows %}{{ peek }}{% endfor %}{{ peek }}",
        "{% weakstash %}{% for row in rows %}{{ weak.name }}{% endfor %}{{ weak }}",
    ],
)
def test_shadow_loop_matches_django(source):
    check(source)


def test_shadow_loop_reports_missing_names_like_django():
    assert (
        check(
            "{% mark %}{% for row in rows %}{{ nothing }}{% endfor %}",
            string_if_invalid="?%s?",
        )
        == "|?nothing??nothing?"
    )


def test_shadow_loop_inside_block_super():
    templates = {
        "base.html": "{% block body %}{% for row in rows %}{{ row.name }}"
        "{% endfor %}{% endblock %}",
        "child.html": "{% extends 'base.html' %}{% load shadow %}{% block body %}"
        "{% mark %}{% for t in 'ab' %}{{ block.super }}{% endfor %}{% endblock %}",
    }
    loaders = [("django.template.loaders.locmem.Loader", templates)]
    expected = (
        DjangoEngine(libraries=LIBRARIES, loaders=loaders)
        .get_template("child.html")
        .render(Context(values()))
    )
    rusty = Engine(libraries=LIBRARIES, loaders=loaders).get_template("child.html")
    assert rusty.render(values()) == expected
    assert rusty.render(Context(values())) == expected
