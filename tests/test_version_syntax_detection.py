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


@pytest.mark.skipif(
    django.VERSION < (6, 0), reason="Modern expressions require Django 6.0"
)
@pytest.mark.parametrize(
    "source,context",
    [
        ("{{ 1e+5 }}", {}),
        ("{% if value == 1E-5 %}yes{% endif %}", {"value": 1e-5}),
        ("{{ value..name }}", {"value": {"": {"name": "empty"}}}),
        ("{{ value. }}", {"value": {"": "trailing"}}),
        ("{{ value|default:1e-5 }}", {}),
        ("{% with value=1e+5 %}{{ value }}{% endwith %}", {}),
        ("{{ 1e+999 }}", {}),
        ("{% if inf %}truthy{% else %}falsey{% endif %}", {}),
        ("{{ nan }}", {"nan": "variable"}),
    ],
)
def test_actual_version_syntax_compiles_natively(monkeypatch, source, context):
    options = dict(
        loaders=[("django.template.loaders.locmem.Loader", {"page.html": source})]
    )
    expected = (
        DjangoEngine(**options)
        .from_string(source)
        .render(django.template.Context(context))
    )
    engine = Engine(**options)

    def unexpected(*args, **kwargs):
        raise AssertionError("supported expressions entered Django parsing")

    monkeypatch.setattr(interop, "compile_template", unexpected)
    assert engine.explain_template(source)["mode"] == "native"
    assert engine.from_string(source).render(context) == expected
    assert engine.get_template("page.html").render(context) == expected


@pytest.mark.skipif(
    django.VERSION < (6, 1), reason="Lookup warnings start in Django 6.1"
)
def test_native_double_dot_warning_matches_django_origin_and_line(tmp_path):
    from django.utils.deprecation import RemovedInDjango2028Warning

    source = "first\n{{ value..name }}"
    (tmp_path / "page.html").write_text(source)
    options = dict(dirs=[tmp_path])
    messages = []
    for kind in [DjangoEngine, Engine]:
        with pytest.warns(RemovedInDjango2028Warning) as caught:
            template = kind(**options).get_template("page.html")
        assert len(caught) == 1
        assert (
            template.render(django.template.Context({"value": {"": {"name": "ok"}}}))
            == "first\nok"
        )
        messages.append(str(caught[0].message))
    assert messages[0] == messages[1]
