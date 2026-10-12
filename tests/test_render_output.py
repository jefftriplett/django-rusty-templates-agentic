"""Output buffers must preserve object protocols and capture boundaries."""

import sys
from datetime import datetime, timezone
from decimal import Decimal
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.utils.functional import lazy
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine


@pytest.fixture
def output_library(monkeypatch):
    module = ModuleType("rusty_output_regression_tags")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    @module.register.simple_tag
    def echo(value):
        return value

    @module.register.filter
    def same_object(value, argument):
        assert value is argument
        return value

    @module.register.simple_tag
    def typed(value, argument):
        assert value is argument
        return value

    @module.register.simple_block_tag
    def capture(content):
        return mark_safe("[" + content + "]")

    return {
        "output": module.__name__,
        "l10n": "django.templatetags.l10n",
        "tz": "django.templatetags.tz",
    }


@pytest.fixture(
    params=[
        (debug, cached, path)
        for debug in (False, True)
        for cached in (False, True)
        for path in ("loaded", "from_string")
    ]
)
def engine_options(request, output_library):
    debug, cached, path = request.param
    return debug, cached, output_library, path


def render(kind, source, values, options, autoescape=True):
    debug, cached, libraries, path = options
    templates = {
        "page.html": source,
        "base.html": "B:{% block body %}{{ value }}{% endblock %}:{{ value }}",
        "part.html": "P:{{ value }}:{{ forloop.counter }}",
    }
    loader = ("django.template.loaders.locmem.Loader", templates)
    engine = kind(
        debug=debug,
        libraries=libraries,
        loaders=[("django.template.loaders.cached.Loader", [loader])]
        if cached
        else [loader],
    )
    context = Context(values, autoescape=autoescape)
    depth = len(context.dicts)
    template = (
        engine.get_template("page.html")
        if path == "loaded"
        else engine.from_string(source)
    )
    result = template.render(context)
    assert len(context.dicts) == depth
    assert context.template is None
    assert "forloop" not in context
    return result


@pytest.mark.parametrize("autoescape", [True, False])
@pytest.mark.parametrize("shape", ["loops", "inheritance", "capture", "typed"])
def test_render_output_boundaries(engine_options, autoescape, shape):
    sources = {
        "loops": "{% for i in items %}{{ value }}{% for j in items %}{% include 'part.html' %}{{ i }}{{ j }}{% endfor %}{% empty %}empty{% endfor %}|{{ value }}",
        "inheritance": "{% extends 'base.html' %}{% block body %}{{ block.super }}{% for i in items %}{% include 'part.html' with value=i %}{% endfor %}{% endblock %}",
        "capture": "{% load output %}{% capture as held %}{% for i in items %}{{ value }}{{ i }}{% endfor %}{% endcapture %}{{ held }}|{% filter upper %}{{ held }}{{ value }}{% endfilter %}|{{ value }}",
        "typed": "{% load output %}{% typed value value as held %}{{ held|same_object:value }}|{% echo value %}|{{ value }}",
    }
    value = mark_safe("<é&>") if shape == "typed" else "<é&>"
    values = {"value": value, "items": ["one", "two"]}
    expected = render(DjangoEngine, sources[shape], values, engine_options, autoescape)
    assert (
        render(Engine, sources[shape], values, engine_options, autoescape) == expected
    )


@pytest.mark.parametrize(
    "kind,autoescape",
    [
        (kind, autoescape)
        for kind in ("exact", "safe", "subclass", "custom", "safe_str", "lazy")
        for autoescape in (True, False)
    ],
)
def test_render_output_string_protocols(engine_options, autoescape, kind):
    def make_value(events):
        class CustomString(str):
            def __str__(self):
                events.append("str")
                return "<str>"

            def __html__(self):
                events.append("html")
                return "<html>"

        class Custom:
            def __str__(self):
                events.append("str")
                return mark_safe("<safe>") if kind == "safe_str" else "<custom>"

            def __html__(self):
                events.append("html")
                return "<html>"

        def deferred():
            events.append("lazy")
            return "<lazy>é🚀"

        return {
            "exact": "<>&\"'é🚀",
            "safe": mark_safe("<>&\"'é🚀"),
            "subclass": CustomString("original"),
            "custom": Custom(),
            "safe_str": Custom(),
            "lazy": lazy(deferred, str)(),
        }[kind]

    direct = (
        "{% echo value %}|"
        if kind in ("exact", "safe", "subclass") or autoescape
        else ""
    )
    source = (
        "{% load output %}{% for i in items %}{{ value }}|"
        + direct
        + "{% echo value as held %}{{ held }};{% endfor %}"
    )
    records = []
    for engine in (DjangoEngine, Engine):
        events = []
        value = make_value(events)
        output = render(
            engine,
            source,
            {"value": value, "items": [1, 2]},
            engine_options,
            autoescape,
        )
        records.append((output, events))
    assert records[1] == records[0]


@pytest.mark.parametrize("protocol", ["str", "html"])
@pytest.mark.parametrize("autoescape", [True, False])
def test_render_output_protocol_errors_preserve_identity(
    engine_options, protocol, autoescape
):
    error = RuntimeError("original output failure")

    class NoisyObject:
        def __str__(self):
            raise error

    class NoisyHtml(str):
        def __html__(self):
            raise error

    value = NoisyObject() if protocol == "str" else NoisyHtml("ok")

    source = "{% for i in items %}prefix:{{ value }}{% endfor %}"
    for kind in (DjangoEngine, Engine):
        if protocol == "html" and not autoescape:
            assert (
                render(
                    kind,
                    source,
                    {"value": value, "items": [1]},
                    engine_options,
                    autoescape,
                )
                == "prefix:ok"
            )
            continue
        with pytest.raises(RuntimeError) as caught:
            render(
                kind,
                source,
                {"value": value, "items": [1]},
                engine_options,
                autoescape,
            )
        assert caught.value is error
        if engine_options[0]:
            assert error.template_debug["during"] == "{{ value }}"


@pytest.mark.parametrize("autoescape", [True, False])
def test_render_output_surrogate_behavior(engine_options, autoescape):
    # Native output has always required valid UTF-8; changing the buffer must
    # neither silently replace a surrogate nor panic on it.
    with pytest.raises(UnicodeEncodeError):
        render(Engine, "{{ value }}", {"value": "\ud800"}, engine_options, autoescape)


def test_render_output_live_formatting_flags(engine_options):
    source = "{% load l10n tz %}{% localize off %}{{ amount }}{% endlocalize %}|{% localize on %}{{ amount }}{% endlocalize %}|{% timezone 'UTC' %}{{ instant }}{% endtimezone %}|{{ amount }}"
    values = {
        "amount": Decimal("1234.50"),
        "instant": datetime(2026, 1, 2, 3, 4, tzinfo=timezone.utc),
    }
    assert render(Engine, source, values, engine_options) == render(
        DjangoEngine, source, values, engine_options
    )


@pytest.mark.parametrize("length", [1023, 1024, 8192])
@pytest.mark.parametrize("suffix", ["", "&", "<", ">", '"', "'"])
@pytest.mark.parametrize("character", ["x", "é"])
def test_large_escape_scan_matches_django(engine_options, length, suffix, character):
    # Include every escape byte at the end, Unicode input, and both sides of
    # the large-string threshold so a fast no-escape scan cannot miss a tail.
    value = character * length + suffix
    source = "{% for i in items %}{{ value }}{% endfor %}"
    values = {"value": value, "items": [1, 2]}
    assert render(Engine, source, values, engine_options) == render(
        DjangoEngine, source, values, engine_options
    )


def test_dense_large_escaping_appends_after_existing_output(engine_options):
    value = "<>&\"'é🚀" * 1024
    source = "prefix:{{ value }}|{{ value }}:tail"
    values = {"value": value}
    assert render(Engine, source, values, engine_options) == render(
        DjangoEngine, source, values, engine_options
    )
