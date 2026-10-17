"""Small reproductions of rendering failures found by Django's admin tests."""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.template.base import TextNode

from django_rusty_templates import Engine


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("inclusion", [False, True])
def test_csrf_survives_custom_nodes_and_inclusion_tags(
    monkeypatch, engine_class, debug, inclusion
):
    module = ModuleType("admin_template_regression_tags")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    @module.register.tag
    def opaque(parser, token):
        return TextNode("")

    @module.register.inclusion_tag("form.html")
    def form():
        return {}

    source = "{% load regression %}{% opaque %}"
    source += "{% form %}" if inclusion else "{% csrf_token %}"
    engine = engine_class(
        debug=debug,
        libraries={"regression": module.__name__},
        loaders=[
            ("django.template.loaders.locmem.Loader", {"form.html": "{% csrf_token %}"})
        ],
    )
    assert engine.from_string(source).render(Context({"csrf_token": "test-token"})) == (
        '<input type="hidden" name="csrfmiddlewaretoken" value="test-token">'
    )
