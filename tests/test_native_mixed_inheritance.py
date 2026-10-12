import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node
from django.template.base import Parser

from django_rusty_templates import Engine


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("dynamic", [False, True])
@pytest.mark.parametrize("cached", [False, True])
def test_python_child_does_not_parse_native_ancestors(
    monkeypatch, debug, dynamic, cached
):
    module = ModuleType("rusty_inheritance_marker")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    class Marker(Node):
        def render(self, context):
            return ""

    @module.register.tag("marker")
    def marker(parser, token):
        return Marker()

    templates = {
        "base.html": "[{% block body %}base {{ value }}{% endblock %}]",
        "middle.html": '{% extends "base.html" %}{% block body %}middle {{ block.super }}{% endblock %}',
        "child.html": (
            "{% extends parent %}" if dynamic else '{% extends "middle.html" %}'
        )
        + "{% load markers %}{% block body %}{% marker %}child {{ block.super }}{% endblock %}",
    }
    loader = ("django.template.loaders.locmem.Loader", templates)
    options = dict(
        debug=debug,
        libraries={"markers": module.__name__},
        loaders=[("django.template.loaders.cached.Loader", [loader])]
        if cached
        else [loader],
    )
    values = {"value": "<&>"}
    expected = (
        DjangoEngine(**options)
        .get_template("child.html")
        .render(Context({**values, "parent": "middle.html"}))
    )
    engine = Engine(**options)
    values["parent"] = engine.get_template("middle.html") if dynamic else "middle.html"
    calls = []
    original = Parser.parse

    def parse(self, *args, **kwargs):
        calls.append(self.origin.template_name)
        assert self.origin.template_name == "child.html", (
            "native ancestor was parsed by Django"
        )
        return original(self, *args, **kwargs)

    monkeypatch.setattr(Parser, "parse", parse)
    template = engine.get_template("child.html")
    context = Context(values)
    depth = len(context.dicts)
    for _ in range(2):
        assert template.render(context) == expected
        assert len(context.dicts) == depth
        assert context.template is None
        assert "block" not in context
    assert calls
