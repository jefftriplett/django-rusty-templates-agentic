import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
@pytest.mark.parametrize("value,expected", [(None, ""), (0, ""), ("yes", "yes")])
def test_firstof_context_assignment(engine_class, value, expected):
    data = {"value": value}
    context = Context(data)
    assert (
        engine_class().from_string("{% firstof value as result %}").render(context)
        == ""
    )
    assert context["result"] == expected
    assert data["result"] == expected


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
def test_scoped_assignments_do_not_leak(engine_class):
    context = Context({"value": "yes"})
    engine_class().from_string(
        "{% with local=value %}{% firstof local as result %}{% endwith %}"
    ).render(context)
    assert "result" not in context
    assert "local" not in context


def test_render_to_string_updates_input_dict():
    data = {"value": "yes"}
    engine = Engine(
        loaders=[
            (
                "django.template.loaders.locmem.Loader",
                {"test.html": "{% firstof value as result %}"},
            )
        ]
    )
    assert engine.render_to_string("test.html", data) == ""
    assert data["result"] == "yes"
