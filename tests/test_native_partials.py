import django
import pytest
from django.template import Context, Engine as DjangoEngine
from django.template.base import Parser

from django_rusty_templates import Engine, interop

pytestmark = pytest.mark.skipif(
    django.VERSION < (6, 0), reason="Partials require Django 6.0"
)


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("cached", [False, True])
def test_partial_definitions_calls_and_named_loading_stay_native(
    monkeypatch, debug, cached
):
    templates = {
        "base.html": "[{% block body %}base{% endblock %}]",
        "page.html": '{% extends "base.html" %}{% partialdef card %}<{{ value }}>{% endpartialdef card %}{% block body %}{% partial card %}|{% include "parts.html#part" %}{% endblock %}',
        "parts.html": "é{% partialdef part %}{% partial nested %}{% endpartialdef %}{% partialdef nested %}{{ value }}{% endpartialdef %}",
    }
    loader = ("django.template.loaders.locmem.Loader", templates)
    options = dict(
        debug=debug,
        loaders=[("django.template.loaders.cached.Loader", [loader])]
        if cached
        else [loader],
    )
    values = {"value": "<&>"}
    expected = DjangoEngine(**options).get_template("page.html").render(Context(values))
    engine = Engine(**options)

    def unexpected(*args, **kwargs):
        raise AssertionError("partials used Django Parser")

    monkeypatch.setattr(interop, "compile_template", unexpected)
    monkeypatch.setattr(Parser, "parse", unexpected)
    assert engine.explain_template(templates["page.html"])["mode"] == "native"
    for _ in range(2):
        assert engine.get_template("page.html").render(Context(values)) == expected
        assert (
            engine.get_template("parts.html#part").render(Context(values))
            == "&lt;&amp;&gt;"
        )
        assert (
            engine.get_template_native("parts.html#part").render(Context(values))
            == "&lt;&amp;&gt;"
        )
    partials = engine.get_template("parts.html").extra_data["partials"]
    assert list(partials) == ["part", "nested"]
    if debug:
        assert (
            partials["part"].source
            == "{% partialdef part %}{% partial nested %}{% endpartialdef %}"
        )


@pytest.mark.parametrize(
    "source",
    [
        '{% partial card %}{% partialdef card %}{% cycle "a" "b" %}{% endpartialdef %}{% partial card %}',
        "{% partialdef card inline %}{{ value }}{% endpartialdef %}|{% partial card %}",
        "{% partialdef card %}{% firstof value as chosen %}{% endpartialdef %}{% partial card %}{{ chosen }}",
        "{% partialdef card %}{% ifchanged value %}x{% endifchanged %}{% endpartialdef %}{% partial card %}{% partial card %}",
    ],
)
def test_partial_render_state_and_assignments_match_django(source):
    values = {"value": "ok"}
    outputs = []
    for kind in [DjangoEngine, Engine]:
        context = Context(values)
        depth = len(context.dicts)
        outputs.append(kind().from_string(source).render(context))
        assert len(context.dicts) == depth
        assert context.template is None
    assert outputs[0] == outputs[1]


@pytest.mark.parametrize("debug", [False, True])
def test_native_partial_registry_survives_detached_mixed_block_bodies(
    monkeypatch, debug
):
    import sys
    from types import ModuleType
    from django.template import Library, Node

    module = ModuleType("rusty_partial_marker")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    class Marker(Node):
        def render(self, context):
            return ""

    @module.register.tag("marker")
    def marker(parser, token):
        return Marker()

    templates = {
        "base.html": "{% partialdef card %}{{ value }}{% endpartialdef %}[{% block body %}{% partial card %}{% endblock %}]",
        "child.html": '{% extends "base.html" %}{% load markers %}{% block body %}{% marker %}child {{ block.super }}{% endblock %}',
    }
    options = dict(
        debug=debug,
        libraries={"markers": module.__name__},
        loaders=[("django.template.loaders.locmem.Loader", templates)],
    )
    results = [
        kind(**options).get_template("child.html").render(Context({"value": "<&>"}))
        for kind in [DjangoEngine, Engine]
    ]
    assert results[0] == results[1] == "[child &lt;&amp;&gt;]"
