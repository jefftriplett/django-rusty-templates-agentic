import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django_rusty_templates import Engine


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("cached", [False, True])
@pytest.mark.parametrize("loaded", [False, True])
def test_inclusion_child_keeps_its_join_boundary_and_context_isolation(
    monkeypatch, debug, cached, loaded
):
    outcomes = []
    for cls in (DjangoEngine, Engine):
        events = []
        module = ModuleType("inclusion_streaming_tags")
        module.register = Library()
        monkeypatch.setitem(sys.modules, module.__name__, module)

        @module.register.simple_tag
        def invalid():
            return 42

        @module.register.simple_tag
        def probe(value):
            events.append(value)
            return ""

        @module.register.inclusion_tag("child.html")
        def component(value):
            events.append("component:" + value)
            return {"value": value}

        templates = {
            "parent.html": "{% load tags %}A{% component value='inner' %}{% probe 'parent' %}",
            "child.html": "{% load tags %}x{% invalid %}{% probe value %}",
        }
        loader = ("django.template.loaders.locmem.Loader", templates)
        engine = cls(
            debug=debug,
            libraries={"tags": module.__name__},
            loaders=[("django.template.loaders.cached.Loader", [loader])]
            if cached
            else [loader],
        )
        context = Context({"value": "outer"}, autoescape=False)
        depth = len(context.dicts)
        template = (
            engine.get_template("parent.html")
            if loaded
            else engine.from_string(templates["parent.html"])
        )
        with pytest.raises(TypeError) as caught:
            template.render(context)
        outcomes.append((str(caught.value), list(events)))
        assert context["value"] == "outer"
        assert len(context.dicts) == depth
        assert context.template is None
    assert (
        outcomes
        == [
            (
                "sequence item 2: expected str instance, int found",
                ["component:inner", "inner"],
            )
        ]
        * 2
    )
