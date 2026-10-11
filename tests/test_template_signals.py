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
