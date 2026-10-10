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

    monkeypatch.setattr("django.urls.reverse", lambda *args, **kwargs: "/a&b'")
    result = render_url("example", None, (), {}, None)
    assert type(result) is str
    assert result == "/a&b'"
