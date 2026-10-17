import sys
import threading
from types import ModuleType

from django import VERSION
import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node
from django.template import TemplateSyntaxError
from inline_snapshot import snapshot

from django_rusty_templates import Engine, RustyTemplates


def test_extends_locmem_loader(engine_class):
    template = "{{ }}"
    loaders = [
        (
            "django.template.loaders.locmem.Loader",
            {"template": template},
        ),
    ]
    config = {
        "OPTIONS": {"loaders": loaders},
        "NAME": "locmem",
        "DIRS": (),
        "APP_DIRS": False,
    }
    engine = engine_class(config)

    with pytest.raises(TemplateSyntaxError) as exc_info:
        engine.get_template("template")

    expected = "Empty variable tag on line 1"
    if VERSION >= (6, 0) and engine_class != RustyTemplates:
        expected = "Template: template, " + expected
    if engine_class == RustyTemplates:
        assert str(exc_info.value) == expected
        assert exc_info.value.rusty_diagnostic == snapshot("""\
  × Empty variable tag
   ╭─[template:1:1]
 1 │ {{ }}
   · ──┬──
   ·   ╰── here
   ╰────
""")
    else:
        assert str(exc_info.value) == expected


def test_tag_compile_function_can_load_from_the_same_engine(monkeypatch):
    module = ModuleType("rusty_loader_reentry_tests")
    module.register = Library()
    current = {}

    class PreloadNode(Node):
        def __init__(self, template):
            self.template = template

        def render(self, context):
            return self.template.render(context)

    @module.register.tag
    def preload(parser, token):
        return PreloadNode(current["engine"].get_template("b.html"))

    monkeypatch.setitem(sys.modules, module.__name__, module)
    templates = {"a.html": "{% load preload %}A{% preload %}", "b.html": "B"}
    locmem = [("django.template.loaders.locmem.Loader", templates)]
    options = {
        "libraries": {"preload": module.__name__},
        "loaders": [("django.template.loaders.cached.Loader", locmem)],
    }

    def render(engine):
        current["engine"] = engine
        return engine.get_template("a.html").render(Context())

    expected = render(DjangoEngine(**options))
    # A daemon thread lets a deadlock fail this test instead of hanging the suite.
    result = {}
    thread = threading.Thread(
        target=lambda: result.update(output=render(Engine(**options))), daemon=True
    )
    thread.start()
    thread.join(timeout=10)
    assert not thread.is_alive(), "get_template deadlocked"
    assert result == {"output": expected}
