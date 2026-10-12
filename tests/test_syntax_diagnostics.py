import django
import pytest
from django.template import Engine as DjangoEngine, TemplateSyntaxError

from django_rusty_templates import Engine


@pytest.mark.parametrize(
    "source",
    [
        "{% unknown_tag %}",
        "{{ value|unknown_filter }}",
        "{% for a b in values %}{% endfor %}",
        "{% if value %}{% else if other %}{% endif %}",
        "{% load from library %}",
        "{% cycle %}",
        "{{ }}",
    ],
)
def test_syntax_exception_message_matches_django(source):
    errors = []
    for engine in [DjangoEngine(), Engine()]:
        with pytest.raises(TemplateSyntaxError) as error:
            engine.from_string(source)
        errors.append(error.value)
    assert str(errors[1]) == str(errors[0])
    assert errors[1].rusty_diagnostic
    assert errors[1].__notes__


def test_successful_compile_does_not_validate_twice(monkeypatch):
    from django_rusty_templates import diagnostics

    def unexpected_validation(*args):
        raise AssertionError("valid native templates must compile once")

    monkeypatch.setattr(diagnostics, "validate_syntax", unexpected_validation)
    assert (
        Engine().from_string("Hello {{ value }}").render({"value": "world"})
        == "Hello world"
    )


def test_attaching_native_diagnostic_does_not_validate_again(monkeypatch):
    from django_rusty_templates import diagnostics

    def unexpected(*args, **kwargs):
        raise AssertionError("diagnostic attachment validated syntax again")

    monkeypatch.setattr(diagnostics, "validate_syntax", unexpected)
    diagnostic = Engine()._syntax_diagnostic("{{ value|unknown_filter }}", None, None)
    assert diagnostic


@pytest.mark.skipif(
    django.VERSION < (6, 0), reason="Signed exponents require Django 6.0"
)
def test_compatibility_error_invokes_custom_compiler_once(monkeypatch):
    import sys
    from types import ModuleType
    import django
    from django.template import Library

    module = ModuleType("rusty_failing_compiler")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)
    calls = []
    original = TemplateSyntaxError("custom failure")

    @module.register.tag("broken")
    def broken(parser, token):
        calls.append(token.contents)
        raise original

    engine = Engine(libraries={"failing": module.__name__}, debug=True)
    monkeypatch.setattr(django, "VERSION", (6, 0, 0, "final", 0))
    with pytest.raises(TemplateSyntaxError) as caught:
        engine.from_string("{{ 1e-5 }}{% load failing %}{% broken %}")
    assert caught.value is original
    assert calls == ["broken"]
    assert original.template_debug["during"] == "{% broken %}"
