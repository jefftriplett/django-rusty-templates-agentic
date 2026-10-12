import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node
from django.template.base import VariableNode

from django_rusty_templates import Engine
from django_rusty_templates.interop import InteropTemplate, NativeVariableNode


@pytest.fixture
def mixed_options(monkeypatch):
    module = ModuleType("rusty_debug_mixed_tags")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    class Marker(Node):
        def render(self, context):
            return ""

    @module.register.tag("marker")
    def compile_marker(parser, token):
        return Marker()

    @module.register.simple_tag
    def badge(value):
        return f"badge:{value}"

    @module.register.inclusion_tag("card.html")
    def card(value):
        return {"value": value}

    return {"mixed": module.__name__}


@pytest.mark.parametrize("cached", [False, True])
def test_debug_inherited_components_do_not_compile_variable_fragments(
    monkeypatch, mixed_options, cached
):
    templates = {
        "base.html": "{% load mixed %}{% marker %}<main>{% block body %}base{% endblock %}</main>",
        "child.html": '{% extends "base.html" %}{% block body %}{% load mixed %}{% marker %}{% for value in values %}{{ value }}{% badge value %}{% card value %}{% include "part.html" %}{% url "home" %}{% endfor %}{% endblock %}',
        "card.html": "{% load mixed %}{% marker %}card:{{ value }}",
        "part.html": "{% load mixed %}{% marker %}part:{{ value }}",
    }
    loader = ("django.template.loaders.locmem.Loader", templates)
    loaders = (
        [("django.template.loaders.cached.Loader", [loader])] if cached else [loader]
    )
    options = dict(debug=True, libraries=mixed_options, loaders=loaders)
    values = {"values": ["<&>", "two"]}
    expected = (
        DjangoEngine(**options).get_template("child.html").render(Context(values))
    )

    def unexpected(*args, **kwargs):
        raise AssertionError(
            "DEBUG mixed template compiled/rendered native variable fragments"
        )

    monkeypatch.setattr(InteropTemplate, "compile_native_nodes", unexpected)
    monkeypatch.setattr(NativeVariableNode, "render", unexpected)
    engine = Engine(**options)
    for _ in range(2):
        template = engine.get_template("child.html")
        assert template.render(Context(values)) == expected
        assert not template.nodelist.get_nodes_by_type(NativeVariableNode)
        assert all(
            type(node) is VariableNode
            for node in template.nodelist.get_nodes_by_type(VariableNode)
        )


@pytest.mark.parametrize("cached", [False, True])
def test_debug_mixed_error_preserves_identity_source_and_scope(mixed_options, cached):
    source = '{% extends "base.html" %}\n{% block body %}{% load mixed %}{% marker %}é{% for value in values %}{{ noisy.value }}{% endfor %}{% endblock %}'
    templates = {
        "base.html": "{% block body %}base{% endblock %}",
        "child.html": source,
    }
    loader = ("django.template.loaders.locmem.Loader", templates)
    loaders = (
        [("django.template.loaders.cached.Loader", [loader])] if cached else [loader]
    )
    error = RuntimeError("original mixed failure")

    class Noisy:
        @property
        def value(self):
            raise error

    context = Context({"values": [1], "noisy": Noisy()})
    depth = len(context.dicts)
    template = Engine(
        debug=True, libraries=mixed_options, loaders=loaders
    ).get_template("child.html")
    with pytest.raises(RuntimeError) as caught:
        template.render(context)
    assert caught.value is error
    assert error.template_debug["during"] == "{{ noisy.value }}"
    assert error.template_debug["name"] == "child.html"
    assert error.template_debug["start"] == source.index("{{ noisy")
    assert len(context.dicts) == depth
    assert context.template is None
    assert "forloop" not in context
    assert "value" not in context


def test_debug_uncached_mixed_template_reads_changed_source(tmp_path, mixed_options):
    source = "{% load mixed %}{% marker %}{{ value }}"
    path = tmp_path / "page.html"
    path.write_text(source)
    loader = ("django.template.loaders.filesystem.Loader", [str(tmp_path)])
    cached = Engine(
        debug=True,
        libraries=mixed_options,
        loaders=[("django.template.loaders.cached.Loader", [loader])],
    )
    uncached = Engine(debug=True, libraries=mixed_options, loaders=[loader])
    assert cached.get_template("page.html").render({"value": "old"}) == "old"
    assert uncached.get_template("page.html").render({"value": "old"}) == "old"
    path.write_text(source + " updated")
    assert cached.get_template("page.html").render({"value": "new"}) == "new"
    assert uncached.get_template("page.html").render({"value": "new"}) == "new updated"
