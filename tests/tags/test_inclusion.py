from types import SimpleNamespace

import pytest
from django.template import (
    Context,
    Engine as DjangoEngine,
    Library,
    TemplateSyntaxError,
)

from django_rusty_templates import Engine
from tests.templatetags import custom_tags as inclusion_tags


@pytest.fixture(params=[Engine, DjangoEngine])
def engine_class(request):
    return request.param


def make_engine(engine_class):
    return engine_class(
        loaders=[
            (
                "django.template.loaders.locmem.Loader",
                {
                    "result.html": "{{ value }}{{ suffix }}{{ extra }}|{{ outer }}|{{ csrf_token }}"
                },
            )
        ],
        libraries={"inclusion_tags": "tests.templatetags.custom_tags"},
    )


@pytest.mark.parametrize(
    "autoescape,expected", [(True, "&lt;b&gt;!x||token"), (False, "<b>!x||token")]
)
def test_inclusion_scope_and_escape(engine_class, autoescape, expected):
    template = make_engine(engine_class).from_string(
        '{% load inclusion_tags %}{% show value extra="x" %}'
    )
    context = Context(
        {"value": "<b>", "outer": "hidden", "csrf_token": "token"},
        autoescape=autoescape,
    )
    assert template.render(context) == expected
    assert context["outer"] == "hidden"


def test_inclusion_context(engine_class):
    template = make_engine(engine_class).from_string(
        "{% load inclusion_tags %}{% from_context %}"
    )
    assert template.render(Context({"outer": "value"})) == "value!||"


@pytest.mark.parametrize("kind", ["name", "names", "template", "backend"])
def test_inclusion_template_selection(engine_class, kind, monkeypatch):
    engine = make_engine(engine_class)
    filename = {
        "name": "result.html",
        "names": ["missing.html", "result.html"],
        "template": engine.get_template("result.html"),
        "backend": SimpleNamespace(template=engine.get_template("result.html")),
    }[kind]
    register = Library()
    register.inclusion_tag(filename, name="selected")(lambda: {"value": "ok"})
    monkeypatch.setattr(inclusion_tags, "register", register)
    engine = make_engine(engine_class)
    template = engine.from_string("{% load inclusion_tags %}{% selected %}")
    assert template.render(Context()) == "ok||"


@pytest.mark.parametrize(
    "source", ["{% show %}", "{% show 1 unknown=2 %}", "{% show 1 as result %}"]
)
def test_inclusion_argument_errors(engine_class, source):
    with pytest.raises(TemplateSyntaxError):
        make_engine(engine_class).from_string("{% load inclusion_tags %}" + source)


@pytest.mark.parametrize("value,expected", [("1|add:1", "2!||"), ('"a"|upper', "A!||")])
def test_inclusion_filtered_literals(engine_class, value, expected):
    template = make_engine(engine_class).from_string(
        "{% load inclusion_tags %}{% show " + value + " %}"
    )
    assert template.render(Context()) == expected


def test_inclusion_preserves_flags(engine_class):
    engine = engine_class(
        loaders=[
            (
                "django.template.loaders.locmem.Loader",
                {"result.html": "{% load inclusion_tags %}{% inclusion_flags %}"},
            )
        ],
        libraries={"inclusion_tags": "tests.templatetags.custom_tags"},
    )
    template = engine.from_string("{% load inclusion_tags %}{% show 1 %}")
    context = Context(use_l10n=True, use_tz=False, autoescape=False)
    assert template.render(context) == "True/False/False"


def test_inclusion_caches_within_render(engine_class, tmp_path, monkeypatch):
    filename = tmp_path / "result.html"
    filename.write_text("first")
    register = Library()
    calls = []

    @register.inclusion_tag("result.html")
    def cached():
        calls.append(True)
        if len(calls) == 2:
            filename.write_text("second")
        return {}

    monkeypatch.setattr(inclusion_tags, "register", register)
    engine = engine_class(
        dirs=[tmp_path],
        loaders=["django.template.loaders.filesystem.Loader"],
        libraries={"inclusion_tags": "tests.templatetags.custom_tags"},
    )
    template = engine.from_string(
        "{% load inclusion_tags %}{% for x in values %}{% cached %}{% endfor %}"
    )
    assert template.render(Context({"values": [1, 2]})) == "firstfirst"
    assert len(calls) == 2
    assert template.render(Context({"values": [1]})) == "second"
