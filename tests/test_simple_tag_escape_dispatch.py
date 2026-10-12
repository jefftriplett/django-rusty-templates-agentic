import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.utils.functional import lazy
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine


@pytest.fixture
def tag_module(monkeypatch):
    module = ModuleType("escape_dispatch_tags")
    module.register = Library()

    @module.register.simple_tag
    def echo(value):
        return value

    @module.register.simple_tag
    def same(value, expected):
        return "same" if value is expected else "different"

    monkeypatch.setitem(sys.modules, module.__name__, module)
    return module


class ProtocolValue:
    def __init__(self, calls):
        self.calls = calls

    def __html__(self):
        self.calls.append("html")
        return "<html>"

    def __str__(self):
        self.calls.append("str")
        return "<string>"


class StringValue:
    def __init__(self, calls):
        self.calls = calls

    def __str__(self):
        self.calls.append("str")
        return "<string>"


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("cached", [False, True])
@pytest.mark.parametrize("loaded", [False, True])
@pytest.mark.parametrize("autoescape", [False, True])
@pytest.mark.parametrize(
    "kind", ["unsafe", "safe", "lazy", "html", "str", "int", "none"]
)
def test_escape_protocol_matches_django(
    tag_module, debug, cached, loaded, autoescape, kind, request
):
    if not autoescape and kind not in ("unsafe", "safe"):
        request.applymarker(
            pytest.mark.xfail(
                strict=True,
                reason="Issue #134: existing autoescape-off non-string result coercion differs from Django",
            )
        )
    source = "{% load escape_dispatch_tags %}{% echo value %}"
    loaders = [("django.template.loaders.locmem.Loader", {"page.html": source})]
    if cached:
        loaders = [("django.template.loaders.cached.Loader", loaders)]
    outputs = []
    protocols = []
    for engine_class in (DjangoEngine, Engine):
        calls = []
        value = {
            "unsafe": lambda: "<unsafe>&",
            "safe": lambda: mark_safe("<safe>"),
            "lazy": lambda: lazy(lambda: "<lazy>", str)(),
            "html": lambda: ProtocolValue(calls),
            "str": lambda: StringValue(calls),
            "int": lambda: 42,
            "none": lambda: None,
        }[kind]()
        engine = engine_class(
            debug=debug,
            loaders=loaders,
            libraries={tag_module.__name__: tag_module.__name__},
        )
        template = (
            engine.get_template("page.html") if loaded else engine.from_string(source)
        )
        try:
            outputs.append(
                template.render(Context({"value": value}, autoescape=autoescape))
            )
        except TypeError as error:
            outputs.append(type(error))
        protocols.append(calls)
    assert outputs[0] == outputs[1]
    assert protocols[0] == protocols[1]


@pytest.mark.parametrize("engine_class", [DjangoEngine, Engine])
def test_asvar_retains_identity_without_protocol_calls(tag_module, engine_class):
    calls = []
    value = ProtocolValue(calls)
    template = engine_class(
        libraries={tag_module.__name__: tag_module.__name__}
    ).from_string(
        "{% load escape_dispatch_tags %}{% echo value as assigned %}{% same assigned value %}"
    )
    assert template.render(Context({"value": value})) == "same"
    assert calls == []


@pytest.mark.parametrize("engine_class", [DjangoEngine, Engine])
def test_html_exception_preserved(tag_module, engine_class):
    error = ValueError("html failure")

    class Broken:
        def __html__(self):
            raise error

    template = engine_class(
        libraries={tag_module.__name__: tag_module.__name__}
    ).from_string("{% load escape_dispatch_tags %}{% echo value %}")
    with pytest.raises(ValueError) as caught:
        template.render(Context({"value": Broken()}))
    assert caught.value is error


def test_repeated_tag_results_reuse_escape_callable():
    # A fresh interpreter keeps this independent of process-global service caches.
    import subprocess

    source = """
import sys
from types import ModuleType
from django.conf import settings
settings.configure(USE_I18N=False)
from django.template import Context, Library
import django.utils.html as html
from django_rusty_templates import Engine
module = ModuleType("cached_escape_probe")
module.register = Library()
module.register.simple_tag(lambda: "<unsafe>", name="value")
sys.modules[module.__name__] = module
engine = Engine(libraries={module.__name__: module.__name__})
template = engine.from_string("{% load cached_escape_probe %}{% value %}")
class TrackingModule(ModuleType):
    lookups = 0
    def __getattribute__(self, name):
        if name == "conditional_escape":
            type(self).lookups += 1
        return super().__getattribute__(name)
html.__class__ = TrackingModule
assert template.render(Context()) == "&lt;unsafe&gt;"
first = TrackingModule.lookups
assert first == 1, first
for _ in range(5):
    assert template.render(Context()) == "&lt;unsafe&gt;"
assert TrackingModule.lookups == first
"""
    subprocess.run([sys.executable, "-c", source], check=True)
