import traceback

import pytest
from django.template import TemplateSyntaxError

from django_rusty_templates import Engine


@pytest.mark.parametrize("debug", [False, True])
def test_render_preserves_exception_identity_and_traceback(debug):
    error = ZeroDivisionError("original failure")
    cause = ValueError("cause")

    class Noisy:
        @property
        def value(self):
            raise error from cause

    source = "é before {{ noisy.value }} after"
    with pytest.raises(ZeroDivisionError) as caught:
        Engine(debug=debug).from_string(source).render({"noisy": Noisy()})
    assert caught.value is error
    assert caught.value.__cause__ is cause
    assert "value" in [
        frame.name for frame in traceback.extract_tb(error.__traceback__)
    ]
    if debug:
        assert error.template_debug["during"] == "{{ noisy.value }}"
        assert error.template_debug["start"] == source.index("{{")
        assert error.template_debug["end"] == source.index("}}") + 2
    else:
        assert not hasattr(error, "template_debug")


@pytest.mark.parametrize("debug", [False, True])
def test_compile_debug_metadata(debug):
    source = "before {{ value|invalid_filter }} after"
    with pytest.raises(TemplateSyntaxError) as caught:
        Engine(debug=debug).from_string(source)
    if debug:
        assert caught.value.template_debug["during"] == "{{ value|invalid_filter }}"
    else:
        assert not hasattr(caught.value, "template_debug")


def test_url_render_bridge_keeps_result_unsafe(monkeypatch):
    from django_rusty_templates.diagnostics import render_url

    monkeypatch.setattr(
        "django_rusty_templates.diagnostics.reverse", lambda *args, **kwargs: "/a&b'"
    )
    result = render_url("example", None, (), {}, None)
    assert type(result) is str
    assert result == "/a&b'"


def test_successful_url_reversal_does_not_construct_context(monkeypatch):
    from django.template import Context
    from django_rusty_templates.diagnostics import render_url

    def unexpected(*args, **kwargs):
        raise AssertionError("Successful URL reversal constructed a Context")

    monkeypatch.setattr(Context, "_reset_dicts", unexpected)
    monkeypatch.setattr(
        "django_rusty_templates.diagnostics.reverse", lambda *args, **kwargs: "/ok/"
    )
    assert render_url("example", None, (), {}, None) == "/ok/"


def test_url_failure_preserves_exception_and_reverses_once(monkeypatch):
    from django_rusty_templates.diagnostics import render_url

    error = ValueError("resolver failed")
    calls = []

    def failing_reverse(*args, **kwargs):
        calls.append(1)
        raise error

    monkeypatch.setattr("django_rusty_templates.diagnostics.reverse", failing_reverse)
    with pytest.raises(ValueError) as caught:
        render_url("example", None, (), {}, None)
    assert caught.value is error
    assert len(calls) == 1
    assert "failing_reverse" in [
        frame.name for frame in traceback.extract_tb(error.__traceback__)
    ]


@pytest.mark.parametrize("loaded", [False, True])
def test_native_exception_info_does_not_parse_a_compatibility_tree(monkeypatch, loaded):
    from django.template import Engine as DjangoEngine
    from django.template.base import DebugLexer, Template
    from django_rusty_templates import interop

    source = "first\né {{ value }}\nlast"
    options = dict(
        debug=True,
        loaders=[("django.template.loaders.locmem.Loader", {"page.html": source})],
    )
    engine = Engine(**options)
    template = (
        engine.get_template("page.html") if loaded else engine.from_string(source)
    )
    reference = (
        DjangoEngine(**options).get_template("page.html")
        if loaded
        else DjangoEngine(**options).from_string(source)
    )
    token = next(
        token for token in DebugLexer(source).tokenize() if token.contents == "value"
    )
    error = RuntimeError("original")
    expected = Template.get_exception_info(reference, error, token)

    def unexpected(*args, **kwargs):
        raise AssertionError("debug metadata compiled a Django tree")

    monkeypatch.setattr(interop, "compile_template", unexpected)
    assert template.get_exception_info(error, token) == expected
