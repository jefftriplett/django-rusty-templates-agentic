"""Object lifetime regressions; RSS and allocator benchmarks stay outside CI."""

import gc
import sys
from types import ModuleType
import weakref

import pytest
from django.template import Context, Library, Node


class Marker:
    pass


class TrackedSource(str):
    pass


def make_backend(engine_class, source, *, debug=False, depth=1, libraries=None):
    marker = Marker()
    reference = weakref.ref(marker)
    source = TrackedSource(source)
    source.marker = marker
    loaders = [
        (
            "django.template.loaders.locmem.Loader",
            {"page.html": source, "value.html": "{{ value }}"},
        )
    ]
    for _ in range(depth):
        loaders = [("django.template.loaders.cached.Loader", loaders)]
    return engine_class(
        {
            "NAME": "memory-test",
            "DIRS": [],
            "APP_DIRS": False,
            "OPTIONS": {
                "debug": debug,
                "loaders": loaders,
                "libraries": libraries or {"custom": "tests.templatetags.custom_tags"},
            },
        }
    ), reference


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("depth", [0, 1, 2])
@pytest.mark.parametrize(
    "source, expected",
    [
        ("Hello {{ value }}", "Hello world"),
        ('{% include "value.html" %}', "world"),
        ("{% block body %}{{ value }}{% endblock %}", "world"),
        ("{% load custom %}{% double value %}", "worldworld"),
    ],
)
def test_discarded_engines_are_collected(engine_class, debug, depth, source, expected):
    def render_once():
        backend, reference = make_backend(
            engine_class, source, debug=debug, depth=depth
        )
        template = backend.get_template("page.html")
        assert template.render({"value": "world"}) == expected
        # Exercise the inspection tree and the mutable Django cache view too.
        assert getattr(template, "template", template).nodelist
        if depth:
            assert backend.engine.template_loaders[0].get_template_cache
        return reference

    references = [render_once() for _ in range(20)]
    gc.collect()
    assert all(reference() is None for reference in references)


@pytest.mark.parametrize("keep", ["template", "engine", "loader", "origin", "cache"])
def test_retained_objects_keep_engine_working(engine_class, keep):
    def retain():
        backend, reference = make_backend(engine_class, '{% include "value.html" %}')
        template = backend.get_template("page.html")
        assert template.render({"value": "first"}) == "first"
        native = getattr(template, "template", template)
        return {
            "template": native,
            "engine": native.engine,
            "loader": backend.engine.template_loaders[0],
            "origin": native.origin,
            "cache": backend.engine.template_loaders[0].get_template_cache,
        }[keep], reference

    retained, reference = retain()
    gc.collect()
    assert reference() is not None
    if keep == "engine":
        template = retained.get_template("page.html")
    elif keep == "loader":
        template = retained.get_template("page.html")
    elif keep == "origin":
        template = retained.loader.get_template("page.html")
    elif keep == "cache":
        template = retained["page.html"]
    else:
        template = retained
    assert template.render(Context({"value": "second"})) == "second"
    del template, retained
    gc.collect()
    assert reference() is None


@pytest.mark.parametrize("debug", [False, True])
def test_opaque_callback_capturing_engine_is_collected(
    engine_class, monkeypatch, debug
):
    module = ModuleType("rusty_memory_callback")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)
    compilations = []

    class CapturingNode(Node):
        def __init__(self, engine, nodes):
            self.engine = engine
            self.nodelist = nodes

        def render(self, context):
            return self.nodelist.render(context)

    @module.register.tag("capture")
    def capture(parser, token):
        compilations.append(token.contents)
        # Compilation holds the loader lock; GC must not try to acquire it.
        gc.collect()
        # Django's parser doesn't expose engine; Rusty's opaque parser does.
        engine = getattr(parser, "engine", None)
        nodes = parser.parse(("endcapture",))
        parser.delete_first_token()
        return CapturingNode(engine, nodes)

    def render_once():
        backend, reference = make_backend(
            engine_class,
            "{% load capture %}{% capture %}{{ value }}{% endcapture %}",
            debug=debug,
            libraries={"capture": module.__name__},
        )
        assert backend.get_template("page.html").render({"value": "ok"}) == "ok"
        compiled = len(compilations)
        gc.collect()
        assert backend.get_template("page.html").render({"value": "again"}) == "again"
        assert len(compilations) == compiled
        return reference

    references = [render_once() for _ in range(20)]
    gc.collect()
    assert all(reference() is None for reference in references)
