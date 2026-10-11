import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node

from django.template import TemplateSyntaxError

from django_rusty_templates import Engine


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("cached", [False, True])
def test_inheritance_compiles_and_renders_without_python_parsing(
    monkeypatch, debug, cached
):
    templates = {
        "base.html": "<main>{% block body %}base {{ value }}{% endblock %}</main>",
        "middle.html": '{% extends "base.html" %}{% block body %}middle {{ block.super }}{% endblock %}',
        "child.html": (
            '{% extends "middle.html" %}{% block body %}'
            '{% with value="<local>" %}{{ value }}:{{ value|upper }}{% endwith %}/'
            '{% for value in values %}{% include "row.html" with suffix="!" %}{% endfor %}'
            "/{{ block.super }}/{{ block.super }}{% endblock %}"
        ),
        "row.html": "{{ value }}{{ suffix }}",
    }
    loader = ("django.template.loaders.locmem.Loader", templates)
    loaders = (
        [("django.template.loaders.cached.Loader", [loader])] if cached else [loader]
    )
    values = {"value": "<&>", "values": ["<one>", "two"]}
    expected = (
        DjangoEngine(debug=debug, loaders=loaders)
        .get_template("child.html")
        .render(Context(values))
    )
    engine = Engine(debug=debug, loaders=loaders)

    def unexpected(*args, **kwargs):
        raise AssertionError(
            "Native inheritance used a Python parser or fragment compiler"
        )

    monkeypatch.setattr("django.template.base.Parser.parse", unexpected)
    monkeypatch.setattr("django_rusty_templates.interop.compile_template", unexpected)
    for _ in range(2):
        template = engine.get_template("child.html")
        assert template._is_native
        assert template.render(Context(values)) == expected
        assert engine.from_string(templates["child.html"]).render(values) == expected


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize(
    "source",
    [
        "{% block body %}{% autoescape off %}{{ value }}{{ value }}{% endautoescape %}/{{ value }}{% endblock %}",
        '{% block body %}{% firstof value as chosen %}{{ chosen }}{% cycle "a" "b" as item %}{{ item }}{% endblock %}',
        "{% block body %}{% for row in rows %}{% block nested %}{{ forloop.counter }}={{ row }}{% endblock %};{% endfor %}{% endblock %}",
        "{% if show %}{% block body %}{{ value }}{% endblock %}{% endif %}",
        '{% cycle "a" "b" as item silent %}{% block body %}{% with x=1 %}{% cycle item %}{% endwith %}{% endblock %}{{ item }}',
        '{% block body %}{% cycle "a" "b" as item silent %}{% with item="local" %}{% cycle item %}{% endwith %}{{ item }}{% endblock %}',
    ],
)
def test_native_block_scopes_match_django(debug, source):
    values = {"value": "<b>", "rows": ["<a>", "&"], "show": True}
    expected = DjangoEngine(debug=debug).from_string(source).render(Context(values))
    template = Engine(debug=debug).from_string(source)
    assert template._is_native
    assert template.render(Context(values)) == expected


@pytest.mark.parametrize("native_child", [True, False])
def test_inheritance_across_custom_compiler_fallback(monkeypatch, native_child):
    module = ModuleType("native_inheritance_custom_tags")
    module.register = Library()

    class CustomNode(Node):
        def render(self, context):
            return "custom"

    module.register.tag("custom", lambda parser, token: CustomNode())
    monkeypatch.setitem(sys.modules, module.__name__, module)
    parent = "[{% block body %}base {{ value }}{% endblock %}]"
    child = (
        '{% extends "base.html" %}{% block body %}child {{ block.super }}{% endblock %}'
    )
    if native_child:
        parent = "{% load custom %}{% custom %}" + parent
    else:
        child = child.replace("child ", "{% load custom %}{% custom %} child ")
    templates = {"base.html": parent, "child.html": child}
    options = {
        "libraries": {"custom": module.__name__},
        "loaders": [("django.template.loaders.locmem.Loader", templates)],
    }
    values = {"value": "<"}
    expected = (
        DjangoEngine(**options).get_template("child.html").render(Context(values))
    )
    engine = Engine(**options)
    assert engine.get_template("child.html").render(values) == expected


@pytest.mark.parametrize("debug", [False, True])
def test_block_error_retains_child_source_and_restores_context(debug):
    source = '{% extends "base.html" %}\né {% block body %}{{ fail }}{% endblock %}'
    engine = Engine(
        debug=debug,
        loaders=[
            (
                "django.template.loaders.locmem.Loader",
                {
                    "base.html": "{% block body %}base{% endblock %}",
                    "child.html": source,
                },
            )
        ],
    )
    error = RuntimeError("block failed")

    def fail():
        raise error

    context = Context({"fail": fail})
    initial_depth = len(context.dicts)
    with pytest.raises(RuntimeError) as caught:
        engine.get_template("child.html").render(context)
    assert caught.value is error
    assert len(context.dicts) == initial_depth
    assert context.template is None
    assert "block" not in context
    if debug:
        assert error.template_debug["during"] == "{{ fail }}"
        assert error.template_debug["name"] == "child.html"


@pytest.mark.parametrize(
    "body",
    [
        '{% cycle "a" "b" %}',
        "{% ifchanged value %}changed{% else %}same{% endifchanged %}",
    ],
)
def test_repeated_super_calls_share_render_state(body):
    templates = {
        "base.html": "{% block body %}" + body + "{% endblock %}",
        "child.html": '{% extends "base.html" %}{% block body %}{{ block.super }}/{{ block.super }}{% endblock %}',
    }
    loaders = [("django.template.loaders.locmem.Loader", templates)]
    values = {"value": "one"}
    expected = (
        DjangoEngine(loaders=loaders).get_template("child.html").render(Context(values))
    )
    template = Engine(loaders=loaders).get_template("child.html")
    assert template.render(values) == expected
    assert template.render(values) == expected


def test_verbatim_before_extends_is_rejected():
    source = '{% verbatim %}{{ literal }}{% endverbatim %}{% extends "base.html" %}'
    for engine in (DjangoEngine(), Engine()):
        with pytest.raises(TemplateSyntaxError, match="must be the first tag"):
            engine.from_string(source)


def test_missing_parent_has_extends_debug_span():
    source = "é before {% extends missing %}"
    template = Engine(debug=True).from_string(source)
    with pytest.raises(TemplateSyntaxError) as caught:
        template.render()
    assert caught.value.template_debug["during"] == "{% extends missing %}"
