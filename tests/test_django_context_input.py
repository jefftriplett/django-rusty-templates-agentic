"""A plain `Context(dict)` is read lazily instead of flattened (#370), and
renders as Django does for every shape of `Context`."""

import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine

SOURCES = [
    "{{ name }}|{{ True }}|{{ None }}|{{ missing }}",
    "{% with name='inner' %}{{ name }}{% endwith %}{{ name }}",
    "{% for name in items %}{{ name }}{% endfor %}{{ name }}",
    "{{ add }}{{ late }}|{{ change }}{{ name }}|{{ drop }}[{{ name }}]",
    "{% cycle 'a' 'b' as name %}{{ name }}",
    "{% if True %}{{ name|upper }}{% endif %}{{ items|length }}",
]


class Subclass(Context):
    pass


def build():
    data = {"name": "caller", "items": ["a", "b"]}
    data["add"] = lambda: data.update(late="added") or ""
    data["change"] = lambda: data.update(name="changed") or ""
    data["drop"] = lambda: data.pop("name", "") and ""
    return data


def plain():
    return Context(build())


def pushed():
    context = Context(build())
    context.push(name="pushed")
    return context


def updated():
    context = Context(build())
    context.update({"extra": "x"})
    return context


def changed_builtins():
    context = Context(build())
    context.dicts[0]["True"] = "shadowed"
    return context


def subclass():
    return Subclass(build())


def empty():
    return Context()


SHAPES = [plain, pushed, updated, changed_builtins, subclass, empty]


@pytest.mark.parametrize("shape", SHAPES, ids=lambda shape: shape.__name__)
@pytest.mark.parametrize("source", SOURCES)
def test_context_shapes_match_django(request, shape, source):
    if source.startswith("{{ add }}") and shape in (
        updated,
        changed_builtins,
        subclass,
    ):
        # Other shapes are still flattened up front, so a dict changed while
        # rendering isn't seen, as it is in Django and for `Context(dict)`.
        request.applymarker(
            pytest.mark.xfail(strict=True, reason="flattened before rendering")
        )
    expected_context = shape()
    expected = DjangoEngine().from_string(source).render(expected_context)
    context = shape()
    assert Engine().from_string(source).render(context) == expected
    # Assignments made by the template are written back as Django leaves them.
    assert values(context) == values(expected_context)


def values(context):
    return {
        key: value for key, value in context.flatten().items() if not callable(value)
    }


def test_caller_dict_is_read_live():
    data = {"name": "before"}
    context = Context(data)
    template = Engine().from_string("{{ name }}")
    assert template.render(context) == "before"
    data["name"] = "after"
    assert template.render(context) == "after"


CALLBACKS = {
    "set": lambda context: context.__setitem__("name", "set") or "",
    "delete": lambda context: context.dicts[-1].pop("name", None) and "",
    "push": lambda context: context.push(name="pushed") and "",
    "escape": lambda context: setattr(context, "autoescape", False) or "",
    "template": lambda context: str(context.template is not None),
    "render": lambda context: str(context.render_context.template is not None),
    "read": lambda context: context.get("name", "missing"),
}


@pytest.mark.parametrize("name", sorted(CALLBACKS))
def test_callables_see_the_live_context(name):
    """A callable in a plain `Context` sees it directly, without the native
    context copied in and back (#370)."""
    source = "{{ callback }}|{{ name }}|{{ html }}|{{ callback }}|{{ name }}"

    def render(engine_class):
        context = Context({"name": "caller", "html": "<b>"})
        context["callback"] = lambda: CALLBACKS[name](context)
        output = engine_class().from_string(source).render(context)
        return output, values(context), context.autoescape, len(context.dicts)

    rusty, django = render(Engine), render(DjangoEngine)
    if name == "push":
        # Rusty removes contexts a callback leaves pushed, keeping their
        # values; Django leaves them on the stack.
        assert rusty[3] == 2
        rusty, django = rusty[:3], django[:3]
    assert rusty == django


def test_callables_inside_native_scopes_still_see_them():
    source = (
        "{% with name='inner' %}{{ callback }}{% endwith %}"
        "{% for name in items %}{{ callback }}{% endfor %}{{ callback }}"
    )

    def render(engine_class):
        context = Context({"name": "caller", "items": ["a", "b"]})
        context["callback"] = lambda: context["name"]
        return engine_class().from_string(source).render(context)

    assert render(Engine) == render(DjangoEngine) == "innerabcaller"
