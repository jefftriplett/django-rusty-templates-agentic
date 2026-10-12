import django
import pytest
from django.template import Engine as DjangoEngine

from django_rusty_templates import Engine, interop


@pytest.mark.parametrize("version", [(6, 0, 0, "final", 0), (6, 1, 0, "final", 0)])
@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize(
    "source",
    [
        "Plain e+ e- E+ E- 1e+5 .. text {{ value }}",
        "{# {{ 1e+5 }} {{ value..name }} #}{{ value }}",
        "{% comment %}{{ 1e+5 }} {{ value..name }}{% endcomment %}{{ value }}",
        "{% verbatim %}{{ 1e+5 }} {{ value..name }}{% endverbatim %}{{ value }}",
        '{{ "1e+5 .." }}{{ value|default:"2E-3 .." }}',
        '{% if value == "1e+5 .." %}yes{% endif %}',
    ],
)
def test_literal_version_syntax_stays_native(monkeypatch, version, debug, source):
    options = dict(
        debug=debug,
        loaders=[("django.template.loaders.locmem.Loader", {"page.html": source})],
    )
    expected = (
        DjangoEngine(**options)
        .from_string(source)
        .render(django.template.Context({"value": "ok"}))
    )
    engine = Engine(**options)
    monkeypatch.setattr(django, "VERSION", version)

    def unexpected(*args, **kwargs):
        raise AssertionError("literal source must not enter Django parsing")

    monkeypatch.setattr(interop, "compile_template", unexpected)
    assert engine.explain_template(source)["mode"] == "native"
    assert engine.from_string(source).render({"value": "ok"}) == expected
    assert engine.get_template("page.html").render({"value": "ok"}) == expected


@pytest.mark.parametrize(
    "version,source",
    [
        ((6, 0, 0, "final", 0), "{{ 1e+5 }}"),
        ((6, 0, 0, "final", 0), "{% if value == 1E-5 %}yes{% endif %}"),
        ((6, 1, 0, "final", 0), "{{ value..name }}"),
    ],
)
def test_actual_version_syntax_retains_explicit_fallback(monkeypatch, version, source):
    engine = Engine()
    monkeypatch.setattr(django, "VERSION", version)
    assert engine.explain_template(source)["mode"] == "hybrid"
    marker = RuntimeError("expected compatibility compilation")

    def fallback(*args, **kwargs):
        raise marker

    monkeypatch.setattr(interop, "compile_template", fallback)
    with pytest.raises(RuntimeError) as caught:
        engine.from_string(source)
    assert caught.value is marker
