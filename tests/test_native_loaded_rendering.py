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
