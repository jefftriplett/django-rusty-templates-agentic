"""Templates reuse one render handle instead of copying themselves (#378),
and lookups check for Django contexts by type (#379)."""

import gc
import tracemalloc
from concurrent.futures import ThreadPoolExecutor

import pytest
from django.template import Context, Engine as DjangoEngine, RequestContext
from django.test import RequestFactory

from django_rusty_templates import Engine

TEMPLATES = {
    "base.html": "<{% block body %}base{% endblock %}>",
    "child.html": "{% extends 'base.html' %}{% block body %}{{ name }}"
    "{% include 'row.html' %}{% endblock %}",
    "row.html": "[{{ name }}]",
}


def engines():
    options = {
        "loaders": [
            (
                "django.template.loaders.cached.Loader",
                [("django.template.loaders.locmem.Loader", TEMPLATES)],
            )
        ]
    }
    return DjangoEngine(**options), Engine(**options)


@pytest.mark.parametrize("name", sorted(TEMPLATES))
def test_repeated_renders_from_cached_loader(name):
    django, rusty = engines()
    for value in ["a", "b", "<c>"]:
        expected = django.get_template(name).render(Context({"name": value}))
        held = rusty.get_template(name)
        assert held.render({"name": value}) == expected
        assert held.render({"name": value}) == expected
        assert rusty.get_template(name).render({"name": value}) == expected


def test_first_render_from_several_threads():
    _, rusty = engines()
    template = rusty.get_template("child.html")

    def render(index):
        return template.render({"name": index})

    with ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(render, range(200)))
    assert results == [f"<{i}[{i}]>" for i in range(200)]


def test_rendered_templates_are_freed():
    rusty = Engine()

    def render_many(count):
        for index in range(count):
            rendered = rusty.from_string(f"{{{{ name }}}}{index}").render({"name": "x"})
            assert rendered == f"x{index}"

    render_many(200)
    gc.collect()
    tracemalloc.start()
    try:
        before = tracemalloc.get_traced_memory()[0]
        render_many(2000)
        gc.collect()
        after = tracemalloc.get_traced_memory()[0]
    finally:
        tracemalloc.stop()
    # Each template is a few hundred bytes; keeping them would be ~1 MB.
    assert after - before < 100_000


@pytest.mark.parametrize(
    "make", [Context, RequestContext], ids=["Context", "RequestContext"]
)
def test_context_values_are_still_recognised(make):
    """A Django context as a value: attribute lookups go to the context's
    keys, as in Django, because its type is a `BaseContext`."""
    request = RequestFactory().get("/")
    inner = (
        make(request, {"flatten": "key", "name": "inner"})
        if make is RequestContext
        else make({"flatten": "key", "name": "inner"})
    )
    source = "{{ inner.name }}|{{ inner.flatten }}|{{ inner.missing }}"
    django, rusty = engines()
    expected = django.from_string(source).render(Context({"inner": inner}))
    assert rusty.from_string(source).render({"inner": inner}) == expected
