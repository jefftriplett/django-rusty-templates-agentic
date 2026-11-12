"""Dict contexts are read lazily but render as Django renders them."""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node

from django_rusty_templates import Engine

library = Library()


@library.simple_tag(takes_context=True)
def peek(context, name):
    return f"[{context.get(name, 'missing')}]"


class KeysNode(Node):
    def render(self, context):
        return ",".join(sorted(k for k in context.flatten() if isinstance(k, str)))


@library.tag
def keys(parser, token):
    return KeysNode()


@library.simple_tag(takes_context=True)
def assign(context, name, value):
    context[name] = value
    return ""


class ForgetNode(Node):
    def __init__(self, name):
        self.name = name

    def render(self, context):
        for layer in context.dicts:
            layer.pop(self.name, None)
        return ""


@library.tag
def forget(parser, token):
    return ForgetNode(token.split_contents()[1].strip("'"))


module = ModuleType("lazy_context_tags")
module.register = library
sys.modules[module.__name__] = module
LIBRARIES = {"lazy": module.__name__, "i18n": "django.templatetags.i18n"}
TEMPLATES = {
    "child.html": "<{{ name }}|{{ extra }}|{{ True }}>",
    "base.html": "{% block body %}{{ name }}{% endblock %}",
}
CONTEXT = {
    "name": "<caller>",
    "items": ["a", "b"],
    "True": "shadowed true",
    "None": "shadowed none",
    "extra": "extra",
    "forloop": "caller forloop",
    "block": "caller block",
}
CONTEXT.update((f"setting_{i}", i) for i in range(100))


@pytest.mark.parametrize(
    "source",
    [
        "{{ name }}{{ setting_99 }}{{ True }}{{ None }}{{ False }}{{ missing }}",
        "{% if True %}{{ True }}{% endif %}{% if None == 'shadowed none' %}y{% endif %}",
        "{% with name='inner' %}{{ name }}{% endwith %}{{ name }}",
        "{% with name='a' %}{% with name='b' %}{{ name }}{% endwith %}{{ name }}"
        "{% endwith %}{{ name }}",
        "{% for name in items %}{{ name }}{% endfor %}{{ name }}",
        "{% for name in items %}{% for name in items %}{{ name }}{% endfor %}"
        "{{ name }}{% endfor %}{{ name }}",
        "{% for x in items %}{{ forloop.counter }}{% endfor %}{{ forloop }}",
        "{% for x in items %}{% cycle 'p' 'q' as name %}{% endfor %}{{ name }}",
        "{% for x in missing %}{% empty %}{{ name }}{% endfor %}",
        "{% firstof missing name %}{% firstof setting_0 name %}",
        "{% include 'child.html' %}{% include 'child.html' with name='w' %}",
        "{% extends 'base.html' %}{% block body %}{{ block.super }}|{{ name }}"
        "{% endblock %}",
        "{% load lazy %}{% peek 'name' %}{% peek 'setting_5' %}{% peek 'True' %}",
        "{% load lazy %}{% keys %}",
        "{% load lazy %}{% with name='w' %}{% peek 'name' %}{% keys %}{% endwith %}",
        "{% load lazy %}{% assign 'name' 'set' %}{{ name }}{% peek 'name' %}",
        "{% load lazy %}{% assign 'fresh' 'new' %}{{ fresh }}",
        "{% load lazy %}{% forget 'name' %}[{{ name }}]{% peek 'name' %}",
        "{% load lazy %}{% for x in items %}{% forget 'name' %}[{{ name }}]{% endfor %}"
        "[{{ name }}]",
        "{% load lazy %}{% with name='w' %}{% forget 'name' %}[{{ name }}]{% endwith %}"
        "[{{ name }}]",
        "{% now 'Y' as name %}{{ name|length }}",
        "{% load i18n %}{% translate 'x' as name %}{{ name }}",
        "{{ items|join:name }}",
    ],
)
def test_lazy_context_matches_django(source):
    options = {
        "libraries": LIBRARIES,
        "loaders": [("django.template.loaders.locmem.Loader", TEMPLATES)],
    }
    django = DjangoEngine(**options).from_string(source)
    rusty = Engine(**options).from_string(source)
    expected = django.render(Context(dict(CONTEXT)))
    assert rusty.render(dict(CONTEXT)) == expected


def test_caller_dict_is_not_changed_by_native_scopes():
    context = {"name": "caller", "items": [1, 2]}
    source = "{% with name='w' %}{% endwith %}{% for name in items %}{% endfor %}"
    Engine().from_string(source).render(context)
    assert context == {"name": "caller", "items": [1, 2]}


def test_non_string_keys_are_ignored_like_django():
    context = {1: "one", "a": "x"}
    django = DjangoEngine().from_string("{{ a }}").render(Context(context))
    assert Engine().from_string("{{ a }}").render(context) == django == "x"


@pytest.mark.parametrize(
    "source",
    [
        "{{ late }}|{{ add }}|{{ late }}",
        "{{ name }}|{{ change }}|{{ name }}",
        "{{ name }}|{{ drop }}|[{{ name }}]",
        "{% with name='w' %}{{ change }}{{ name }}{% endwith %}|{{ name }}",
    ],
)
def test_caller_dict_changes_during_render_are_seen_like_django(source):
    """Django reads the caller's dict live, so no snapshot is taken (#378)."""

    def build():
        context = {"name": "caller", "items": [1, 2]}
        context["add"] = lambda: context.update(late="added") or ""
        context["change"] = lambda: context.update(name="changed") or ""
        context["drop"] = lambda: context.pop("name", "") and ""
        return context

    expected = DjangoEngine().from_string(source).render(Context(build()))
    assert Engine().from_string(source).render(build()) == expected


def test_render_returns_safe_string_for_any_output():
    from django.utils.safestring import SafeString

    for source in ["", "plain <b>", "{{ name }}"]:
        rendered = Engine().from_string(source).render({"name": "<x>"})
        assert type(rendered) is SafeString
