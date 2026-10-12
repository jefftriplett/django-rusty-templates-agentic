import pytest
from django.template import Context

from django_rusty_templates import Engine
from django_rusty_templates.interop import NativeVariableNode


@pytest.mark.parametrize("cached", [True, False])
def test_supported_loaded_templates_do_not_render_variable_fragments(
    monkeypatch, cached
):
    def unexpected_fragment(self, context):
        raise AssertionError("Supported loaded template used variable interoperability")

    def unexpected_inspection(*args, **kwargs):
        raise AssertionError("Native cache hit compiled a Django inspection tree")

    monkeypatch.setattr(NativeVariableNode, "render", unexpected_fragment)
    monkeypatch.setattr(
        "django_rusty_templates.interop.compile_template", unexpected_inspection
    )
    templates = {
        "catalog.html": (
            "<ul>{% for row in rows %}<li>{{ row.name }} "
            '{% include "price.html" %}</li>{% endfor %}</ul>'
        ),
        "price.html": "{{ row.price|floatformat:2 }}",
    }
    loader = ("django.template.loaders.locmem.Loader", templates)
    loaders = (
        [("django.template.loaders.cached.Loader", [loader])] if cached else [loader]
    )
    engine = Engine(loaders=loaders)
    context = Context({"rows": [{"name": "<b>", "price": 1.25}]})
    for _ in range(2):
        assert engine.get_template("catalog.html").render(context) == (
            "<ul><li>&lt;b&gt; 1.25</li></ul>"
        )


@pytest.mark.parametrize("cached", [True, False])
@pytest.mark.parametrize("debug", [True, False])
@pytest.mark.parametrize("autoescape", [True, False])
@pytest.mark.parametrize("use_l10n", [True, False, None])
def test_loop_value_formatting_does_not_snapshot_context(
    monkeypatch, cached, debug, autoescape, use_l10n
):
    from datetime import datetime, timezone
    from decimal import Decimal

    from django.template import Engine as DjangoEngine
    from django.utils import translation

    source = (
        '{% extends "base.html" %}{% block content %}'
        "{% for i in values %}{{ i }}|{% endfor %}"
        "{% autoescape off %}{% for i in values %}{{ i }}|{% endfor %}"
        "{% endautoescape %}{% endblock %}"
    )
    templates = {
        "loop.html": source,
        "base.html": "{% block content %}{% endblock %}",
    }
    loader = ("django.template.loaders.locmem.Loader", templates)
    loaders = (
        [("django.template.loaders.cached.Loader", [loader])] if cached else [loader]
    )
    values = {
        "values": [
            1234,
            1.25,
            Decimal("1234.50"),
            datetime(2026, 1, 2, tzinfo=timezone.utc),
            "<&>",
        ]
    }
    options = dict(autoescape=autoescape, use_l10n=use_l10n, use_tz=True)
    with translation.override("de"):
        reference = DjangoEngine(loaders=loaders, debug=debug).get_template("loop.html")
        expected = reference.render(Context(values, **options))
        template = Engine(loaders=loaders, debug=debug).get_template("loop.html")
        context = Context(values, **options)

        def unexpected_snapshot(*args, **kwargs):
            raise AssertionError("Value formatting constructed a Django Context")

        monkeypatch.setattr(Context, "_reset_dicts", unexpected_snapshot)
        assert template.render(context) == expected
