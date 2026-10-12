"""Loaded loop cases for the optional CodSpeed regression dashboard."""

import pytest
from django.template import Context

from django_rusty_templates import Engine


@pytest.mark.parametrize("cached", [True, False])
@pytest.mark.parametrize("body", ["x", "{{ i }}", "{{ i }}{{ i }}"])
def test_loaded_loop_values(benchmark, cached, body):
    source = "{% for i in values %}" + body + "{% endfor %}"
    loader = ("django.template.loaders.locmem.Loader", {"loop.html": source})
    loaders = (
        [("django.template.loaders.cached.Loader", [loader])] if cached else [loader]
    )
    engine = Engine(loaders=loaders)
    template = engine.get_template("loop.html")
    values = list(range(100))
    context = Context({"values": values})
    expected = "".join(
        "x" if body == "x" else str(i) * body.count("{{ i }}") for i in values
    )
    assert template.render(context) == expected
    benchmark(template.render, context)
