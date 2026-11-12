"""A template rendered from inside another template's block starts with its
own block table, as Django's `Template.render` pushes a fresh render context
(#391)."""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node

from django_rusty_templates import Engine

MODULE = "rusty_nested_render_tests"


def engines(templates):
    loaders = [("django.template.loaders.locmem.Loader", templates)]
    return (
        DjangoEngine(loaders=loaders, libraries={"t": MODULE}),
        Engine(loaders=loaders, libraries={"t": MODULE}),
    )


@pytest.fixture
def library(monkeypatch):
    module = ModuleType(MODULE)
    module.register = Library()

    @module.register.simple_tag(takes_context=True)
    def render_other(context, name):
        return module.engine.get_template(name).render(context)

    class Noop(Node):
        def render(self, context):
            return ""

    def noop(parser, token):
        return Noop()

    noop.rusty_isolated_compiler = True
    module.register.tag("noop", noop)
    monkeypatch.setitem(sys.modules, MODULE, module)
    return module


BASE = "<{% block a %}base{% endblock %}>"
CASES = {
    "child extends the same parent": {
        "outer.html": "{% extends 'base.html' %}{% load t %}{% block a %}[{% render_other 'child.html' %}]{% endblock %}",
        "child.html": "{% extends 'base.html' %}{% block a %}child{% endblock %}",
    },
    "child without extends keeps its own block": {
        "outer.html": "{% extends 'base.html' %}{% load t %}{% block a %}[{% render_other 'child.html' %}]{% endblock %}",
        "child.html": "{% block a %}own{% endblock %}",
    },
    "child uses block.super": {
        "outer.html": "{% extends 'base.html' %}{% load t %}{% block a %}[{% render_other 'child.html' %}]{% endblock %}",
        "child.html": "{% extends 'base.html' %}{% block a %}c{{ block.super }}{% endblock %}",
    },
    "outer state is restored after the inner render": {
        "outer.html": "{% extends 'base.html' %}{% load t %}{% block a %}[{% render_other 'child.html' %}]{{ block.super }}{% endblock %}",
        "child.html": "{% extends 'base.html' %}{% block a %}child{% endblock %}",
    },
    "no inheritance outside": {
        "outer.html": "{% load t %}[{% render_other 'child.html' %}]",
        "child.html": "{% extends 'base.html' %}{% block a %}child{% endblock %}",
    },
}


@pytest.mark.parametrize("live", [False, True])
@pytest.mark.parametrize("case", CASES)
def test_nested_render_matches_django(library, case, live):
    templates = {"base.html": BASE, **CASES[case]}
    if live:
        # A custom compiled tag renders the outer template on a live Context.
        templates["outer.html"] = templates["outer.html"].replace(
            "{% load t %}", "{% load t %}{% noop %}"
        )
    django, rusty = engines(templates)
    library.engine = django
    expected = django.get_template("outer.html").render(Context({}))
    library.engine = rusty
    assert rusty.get_template("outer.html").render({}) == expected
    assert rusty.get_template("outer.html").render(Context({})) == expected
