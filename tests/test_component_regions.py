import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node
from django.template.defaulttags import ForNode, IfNode, URLNode
from django.template.library import InclusionNode, SimpleNode
from django.template.loader_tags import IncludeNode

from django_rusty_templates import Engine
from django_rusty_templates.interop import NativeRegionNode


@pytest.mark.parametrize("cached", [True, False])
def test_mixed_components_use_one_region_and_native_control_flow(monkeypatch, cached):
    module = ModuleType("rusty_component_region_tests")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    @module.register.simple_tag
    def badge(value):
        return f"badge:{value}"

    @module.register.inclusion_tag("card.html")
    def card(value):
        return {"value": value}

    class CustomNode(Node):
        def __init__(self, expression):
            self.expression = expression

        def render(self, context):
            context["seen"] = self.expression.resolve(context)
            return f"custom:{context['forloop']['counter']}"

    @module.register.tag("custom")
    def compile_custom(parser, token):
        return CustomNode(parser.compile_filter(token.split_contents()[1]))

    templates = {
        "page.html": (
            "{% load components %}Café:"
            "{% for row in rows %}{% if row.show %}"
            "{% badge row.name %}/{{ row.name }}/"
            "{% custom row.name %}/{{ seen }}/"
            '{% include "part.html" with value=row.name %}/'
            "{% card row.name %}/{% url 'home' %}|"
            "{% endif %}{% endfor %}"
        ),
        "part.html": "part:{{ value }}",
        "card.html": "card:{{ value }}",
    }
    loader = ("django.template.loaders.locmem.Loader", templates)
    loaders = (
        [("django.template.loaders.cached.Loader", [loader])] if cached else [loader]
    )
    options = dict(
        loaders=loaders, libraries={"components": module.__name__}, debug=False
    )
    values = {
        "rows": [
            {"name": "<&>", "show": True},
            {"name": "b", "show": False},
            {"name": "c", "show": True},
        ]
    }
    expected = DjangoEngine(**options).get_template("page.html").render(Context(values))
    template = Engine(**options).get_template("page.html")

    def unexpected(*args, **kwargs):
        raise AssertionError(
            "Supported component/control flow rendered as a Python node"
        )

    for node_type in [ForNode, IfNode, SimpleNode, InclusionNode, IncludeNode, URLNode]:
        monkeypatch.setattr(node_type, "render", unexpected)
    calls = []
    original = NativeRegionNode.render

    def counted(self, context):
        calls.append(self)
        return original(self, context)

    monkeypatch.setattr(NativeRegionNode, "render", counted)
    context = Context(values)
    monkeypatch.setattr(Context, "_reset_dicts", unexpected)
    assert template.render(context) == expected
    assert len(calls) == 1


def test_opaque_compiler_consumed_span_and_lookahead(monkeypatch):
    module = ModuleType("rusty_component_block_tests")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    class Capture(Node):
        child_nodelists = ("nodelist",)

        def __init__(self, nodes):
            self.nodelist = nodes

        def render(self, context):
            context["value"] = "changed"
            return "[" + self.nodelist.render(context) + "]"

    @module.register.tag("capture")
    def compile_capture(parser, token):
        nodes = parser.parse(("endcapture",))
        parser.delete_first_token()
        if parser.tokens:
            lookahead = parser.next_token()
            parser.prepend_token(lookahead)
        return Capture(nodes)

    source = "{% load components %}é{% for i in items %}{% capture %}{{ i }}:{{ value }}{% endcapture %}{{ value }}{% endfor %}tail"
    options = dict(libraries={"components": module.__name__}, debug=False)
    values = {"items": [1, 2], "value": "old"}
    expected = DjangoEngine(**options).from_string(source).render(Context(values))
    template = Engine(**options).from_string(source)
    assert template.render(Context(values)) == expected
    assert template.render(Context(values)) == expected


def test_relative_include_and_named_cycles_in_mixed_template(monkeypatch):
    module = ModuleType("rusty_component_relative_tests")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    class Empty(Node):
        def render(self, context):
            return ""

    @module.register.tag("custom")
    def compile_custom(parser, token):
        return Empty()

    templates = {
        "nested/page.html": (
            "{% load components %}{% custom %}"
            "{% cycle 'a' 'b' as stripe %}/"
            '{% for i in items %}{% cycle stripe %}{% include "./part.html" %}{% endfor %}'
        ),
        "nested/part.html": "{{ i }}",
    }
    options = dict(
        libraries={"components": module.__name__},
        loaders=[("django.template.loaders.locmem.Loader", templates)],
        debug=False,
    )
    values = {"items": [1, 2, 3]}
    expected = (
        DjangoEngine(**options).get_template("nested/page.html").render(Context(values))
    )
    engine = Engine(**options)
    assert engine.get_template("nested/page.html").render(Context(values)) == expected


def test_public_nodelist_preserves_custom_node_indexing():
    template = Engine(
        libraries={"cache": "django.templatetags.cache"}, debug=False
    ).from_string("{% load cache %}{% cache 1 component %}content{% endcache %}")
    assert template.nodelist[1].fragment_name == "component"
    assert len(template.nodelist.rendering_nodes) == 1
