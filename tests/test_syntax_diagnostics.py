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
