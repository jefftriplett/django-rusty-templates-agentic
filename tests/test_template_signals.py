import pytest
from django.shortcuts import render
from django.test import SimpleTestCase, override_settings
from django.test.signals import template_rendered
from django.urls import path

from django_rusty_templates import Engine


def processor(request):
    return {"processed": request.path}


def view(request):
    return render(request, "parent.html", {"name": "world"})


urlpatterns = [path("", view)]


@pytest.mark.parametrize(
    "backend",
    [
        "django.template.backends.django.DjangoTemplates",
        "django_rusty_templates.RustyTemplates",
    ],
)
@pytest.mark.parametrize(
    "parent,expected",
    [
        ('<p>{{ name }}</p>{% include "child.html" %}', ["parent.html", "child.html"]),
        (
            '{% extends "base.html" %}{% block body %}{{ name }}{% include "child.html" %}{% endblock %}',
            ["parent.html", "base.html", "child.html"],
        ),
        (
            '{% include "child.html" with name="child" only %}',
            ["parent.html", "child.html"],
        ),
    ],
)
def test_client_template_and_context_introspection(client, backend, parent, expected):
    templates = {
        "parent.html": parent,
        "child.html": "<p>{{ name }}</p>",
        "base.html": "{% block body %}{% endblock %}",
    }
    config = [
        {
            "BACKEND": backend,
            "OPTIONS": {
                "loaders": [("django.template.loaders.locmem.Loader", templates)],
                "context_processors": ["tests.test_template_signals.processor"],
            },
        }
    ]
    with override_settings(TEMPLATES=config, ROOT_URLCONF=__name__):
        response = client.get("/")
    assert response.status_code == 200
    assert [template.name for template in response.templates] == expected
    assert response.context["name"] == "world"
    assert response.context["processed"] == "/"
    for template in response.templates:
        assert template.origin.template_name == template.name
        SimpleTestCase().assertTemplateUsed(response, template.name, count=1)
    if "only" in parent:
        assert response.context[-1]["name"] == "child"
        assert "processed" not in response.context[-1]


def test_native_signal_payload_and_anonymous_template():
    seen = []

    def receiver(sender, template, context, **kwargs):
        assert sender is template
        seen.append((template, context))

    template_rendered.connect(receiver, weak=False)
    try:
        template = Engine().from_string("{{ value }}")
        assert template.render({"value": "hello"}) == "hello"
    finally:
        template_rendered.disconnect(receiver)
    assert len(seen) == 1
    rendered, context = seen[0]
    assert rendered.name is None
    assert context["value"] == "hello"


@pytest.mark.parametrize("fallback", [False, True])
def test_include_signal_contexts_preserve_scopes(fallback):
    child = "{{ name }}"
    if fallback:
        child = "{% block body %}" + child + "{% endblock %}"
    engine = Engine(
        loaders=[("django.template.loaders.locmem.Loader", {"child.html": child})]
    )
    seen = []

    def receiver(sender, template, context, **kwargs):
        seen.append((template.name, context))

    template_rendered.connect(receiver, weak=False)
    try:
        output = engine.from_string(
            '{% include "child.html" %}'
            '{% for item in items %}{% include "child.html" with name=item %}{% endfor %}'
            '{% with name="scoped" %}{% include "child.html" %}{% endwith %}'
            '{% include "child.html" with name="isolated" only %}'
            '{% include "child.html" %}'
        ).render({"name": "outer", "items": ["first", "second"], "shared": object()})
    finally:
        template_rendered.disconnect(receiver)
    assert output == "outerfirstsecondscopedisolatedouter"
    children = [context for name, context in seen if name == "child.html"]
    assert [context["name"] for context in children] == [
        "outer",
        "first",
        "second",
        "scoped",
        "isolated",
        "outer",
    ]
    assert ["shared" in context for context in children] == [
        True,
        True,
        True,
        True,
        False,
        True,
    ]
    assert "item" not in children[-1]


def test_no_receivers_skips_listener_lookup(monkeypatch):
    def unexpected(*args, **kwargs):
        raise AssertionError("Empty signals should skip receiver lookup")

    monkeypatch.setattr(template_rendered, "receivers", [])
    monkeypatch.setattr(template_rendered, "has_listeners", unexpected)
    assert Engine().from_string("{{ name }}").render({"name": "world"}) == "world"


def test_fallback_context_assignments_do_not_change_later_includes():
    child = (
        "{% load custom_tags %}{% block body %}{% counter %}{{ count }}{% endblock %}"
    )
    engine = Engine(
        libraries={"custom_tags": "tests.templatetags.custom_tags"},
        loaders=[("django.template.loaders.locmem.Loader", {"child.html": child})],
    )
    template = engine.from_string(
        '{{ count }}{% include "child.html" %}'
        '{{ count }}{% include "child.html" %}{{ count }}'
    )
    assert template.render({"count": 0}) == "01010"


@pytest.mark.parametrize(
    "child",
    [
        "{{ value }}:{{ forloop.counter }}:{{ value|upper }}",
        '{% with value="local" %}{{ value }}{% endwith %}:{{ value }}',
    ],
)
def test_inherited_include_keeps_live_context_and_signals(child):
    templates = {
        "base.html": "{% block body %}{% endblock %}",
        "parent.html": (
            '{% extends "base.html" %}{% block body %}'
            '{% for value in values %}{% include "child.html" %};{% endfor %}'
            "{% endblock %}"
        ),
        "child.html": child,
    }
    seen = []

    def receiver(sender, template, context, **kwargs):
        if template.name == "child.html":
            seen.append(context["value"])

    template_rendered.connect(receiver, weak=False)
    try:
        engine = Engine(loaders=[("django.template.loaders.locmem.Loader", templates)])
        output = engine.get_template("parent.html").render({"values": ["one", "two"]})
    finally:
        template_rendered.disconnect(receiver)
    assert output == (
        "local:one;local:two;" if "with" in child else "one:1:ONE;two:2:TWO;"
    )
    assert seen == ["one", "two"]


def test_first_context_snapshot_inside_loop_restores_missing_variables(monkeypatch):
    import sys
    from types import ModuleType

    from django.template import Library, Node

    # Blocks compile natively now. Use an arbitrary compiler to exercise the
    # Python fallback that this snapshot regression specifically targets.
    library = ModuleType("snapshot_fallback_tags")
    library.register = Library()

    class EmptyNode(Node):
        def render(self, context):
            return ""

    library.register.tag("snapshot", lambda parser, token: EmptyNode())
    monkeypatch.setitem(sys.modules, library.__name__, library)
    # Disable native signaling so the first Python snapshot is made for a
    # fallback child after the loop variable has already entered its scope.
    monkeypatch.setattr(template_rendered, "has_listeners", lambda sender=None: False)
    child = '{% load snapshot %}{% snapshot %}{% block body %}{{ item|default:"missing" }}{% endblock %}'
    engine = Engine(
        libraries={"snapshot": library.__name__},
        loaders=[("django.template.loaders.locmem.Loader", {"child.html": child})],
    )
    contexts = []

    def receiver(sender, template, context, **kwargs):
        if template.name == "child.html":
            contexts.append(context)

    template_rendered.connect(receiver, weak=False)
    try:
        output = engine.from_string(
            '{% for item in items %}{% include "child.html" %}{% endfor %}'
            '{% include "child.html" %}{% include "child.html" %}'
        ).render({"items": ["one", "two"], "shared": "outer"})
    finally:
        template_rendered.disconnect(receiver)
    assert output == "onetwomissingmissing"
    assert [context.get("item") for context in contexts] == ["one", "two", None, None]
    assert all(context["shared"] == "outer" for context in contexts)
