"""`takes_context` tags see the right context when Rusty reuses its wrapper.

Rusty reuses the Python wrapper it passes to `takes_context` tags unless the
tag kept a reference to it. These tests keep some contexts and not others,
nest tags, and raise from a tag, then check every context Python saw.
"""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library

from django_rusty_templates import Engine


@pytest.fixture
def tags(monkeypatch):
    module = ModuleType("context_reuse_tags")
    module.register = Library()
    module.kept = []
    monkeypatch.setitem(sys.modules, module.__name__, module)

    @module.register.simple_tag(takes_context=True)
    def show(context, keep=False):
        if keep:
            module.kept.append(context)
        return context["item"]

    @module.register.simple_block_tag(takes_context=True)
    def wrap(context, content):
        return f"[{context['item']}:{content}]"

    @module.register.simple_tag(takes_context=True)
    def fail(context):
        raise ValueError(context["item"])

    return module


def test_kept_contexts_keep_their_values(tags):
    """Rusty-only: a kept Rusty context is a snapshot, while Django's is the
    live `Context`, which loses `item` once the loop ends."""
    engine = Engine(libraries={"tags": tags.__name__})
    source = (
        "{% load tags %}{% for item in items %}"
        "{% show keep=forloop.counter|divisibleby:3 %}{% endfor %}"
    )
    template = engine.from_string(source)
    items = list("abcdefghi")
    assert template.render(Context({"items": items})) == "abcdefghi"
    assert [context["item"] for context in tags.kept] == ["c", "f", "i"]
    assert len({id(context) for context in tags.kept}) == 3
    # A later render reuses wrappers; the kept contexts don't change.
    assert template.render(Context({"items": list("xyz")})) == "xyz"
    assert [context["item"] for context in tags.kept[:3]] == ["c", "f", "i"]


@pytest.mark.parametrize("engine_class", [DjangoEngine, Engine])
def test_nested_context_tags(tags, engine_class):
    engine = engine_class(libraries={"tags": tags.__name__})
    source = (
        "{% load tags %}{% for item in items %}"
        "{% wrap %}{% show %}{% with item='in' %}{% show %}{% endwith %}{% endwrap %}"
        "{% show %}{% endfor %}"
    )
    output = engine.from_string(source).render(Context({"items": ["a", "b"]}))
    assert output == "[a:ain]a[b:bin]b"


@pytest.mark.parametrize("engine_class", [DjangoEngine, Engine])
def test_render_after_tag_raises(tags, engine_class):
    engine = engine_class(libraries={"tags": tags.__name__})
    failing = engine.from_string("{% load tags %}{% fail %}")
    with pytest.raises(ValueError, match="boom"):
        failing.render(Context({"item": "boom"}))
    # Later renders work after a tag raised while holding the context.
    working = engine.from_string("{% load tags %}{% show %}{% show %}")
    assert working.render(Context({"item": "ok"})) == "okok"
