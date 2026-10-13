import pytest
from django.template import Context, Engine as DjangoEngine, TemplateSyntaxError
from django.template.defaultfilters import register
from django.utils.safestring import mark_safe
from django_rusty_templates import Engine


def test_compiled_builtin_filter_retains_callable_after_registry_replacement(
    monkeypatch,
):
    source = "{{ value|pluralize }}"
    original = register.filters["pluralize"]
    for cls in (DjangoEngine, Engine):
        with monkeypatch.context() as patch:
            patch.setitem(register.filters, "pluralize", original)
            engine = cls()
            compiled = engine.from_string(source)

            def replacement(value, suffix="s"):
                return "replacement"

            patch.setitem(register.filters, "pluralize", replacement)
            newer = engine.from_string(source)
            assert compiled.render(Context({"value": 2})) == "s"
            assert newer.render(Context({"value": 2})) == "replacement"
            assert compiled.render(Context({"value": 2})) == "s"


def test_compiled_builtin_filter_keeps_flags_live(monkeypatch):
    for cls in (DjangoEngine, Engine):
        calls = []

        def replacement(value, suffix="s", *, autoescape=True):
            calls.append(autoescape)
            return "<on>" if autoescape else "<off>"

        replacement.is_safe = False
        replacement.needs_autoescape = False
        with monkeypatch.context() as patch:
            patch.setitem(register.filters, "pluralize", replacement)
            template = cls().from_string("{{ value|pluralize }}")
            assert calls == []
            assert template.render(Context({"value": mark_safe("2")})) == "&lt;on&gt;"
            replacement.is_safe = True
            replacement.needs_autoescape = True
            assert template.render(Context({"value": mark_safe("2")})) == "<on>"
            assert (
                template.render(Context({"value": mark_safe("2")}, autoescape=False))
                == "<off>"
            )
            assert calls == [True, True, False]


def test_removed_builtin_filter_is_a_compile_error(monkeypatch):
    monkeypatch.delitem(register.filters, "pluralize")
    for cls in (DjangoEngine, Engine):
        with pytest.raises(TemplateSyntaxError, match="Invalid filter"):
            cls().from_string("{{ value|pluralize }}")
