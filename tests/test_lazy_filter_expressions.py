"""Mixed templates parse native variables lazily but behave as Django does."""

import sys
from types import ModuleType

import pytest
from django.template import (
    Context,
    Engine as DjangoEngine,
    Library,
    Node,
    TemplateSyntaxError,
)
from django.template.base import FilterExpression, VariableNode

from django_rusty_templates import Engine
from django_rusty_templates.interop import LazyFilterExpression, compile_template

library = Library()


class OpaqueNode(Node):
    def render(self, context):
        return "opaque"


@library.tag
def opaque(parser, token):
    return OpaqueNode()


class HiddenBodyNode(Node):
    """A custom node that keeps its body where Django can't see it."""

    child_nodelists = ()

    def __init__(self, body):
        self.body = body

    def render(self, context):
        return self.body.render(context)


@library.tag
def hidden(parser, token):
    body = parser.parse(("endhidden",))
    parser.delete_first_token()
    return HiddenBodyNode(body)


@library.filter
def no_argument(value):
    return value


shout_library = Library()


@shout_library.filter(name="upper")
def shouting(value):
    return f"{value}!"


module = ModuleType("lazy_filter_expressions")
module.register = library
sys.modules[module.__name__] = module
shout_module = ModuleType("lazy_filter_expressions_shout")
shout_module.register = shout_library
sys.modules[shout_module.__name__] = shout_module
LIBRARIES = {"lazy": module.__name__, "shout": shout_module.__name__}
PREFIX = "{% load lazy %}{% opaque %}"


def engines(**options):
    return [
        DjangoEngine(libraries=LIBRARIES, **options),
        Engine(libraries=LIBRARIES, **options),
    ]


def test_native_variables_are_lazy_and_inspect_like_django():
    source = PREFIX + '<p>{{ name|title }} {{ note|default:"-" }}</p>'
    django = DjangoEngine(libraries=LIBRARIES).from_string(source)
    rusty = compile_template(source, Engine(libraries=LIBRARIES))
    expressions = [
        node.filter_expression
        for node in rusty.nodelist.get_nodes_by_type(VariableNode)
    ]
    assert all(type(e) is LazyFilterExpression for e in expressions)
    expected = [
        node.filter_expression
        for node in django.nodelist.get_nodes_by_type(VariableNode)
    ]
    for lazy, eager in zip(expressions, expected, strict=True):
        assert isinstance(lazy, FilterExpression)
        assert str(lazy) == str(eager)
        assert lazy.is_var == eager.is_var
        assert str(lazy.var) == str(eager.var)
        assert lazy.filters == eager.filters
    context = {"name": "ada lovelace", "note": ""}
    assert rusty.render(Context(context)) == django.render(Context(context))


def test_later_load_does_not_change_earlier_variables():
    source = PREFIX + "{{ word|upper }}{% load shout %}{{ word|upper }}"
    django, rusty = (e.from_string(source) for e in engines())
    assert rusty.render(Context({"word": "hi"})) == "opaqueHIhi!"
    assert django.render(Context({"word": "hi"})) == "opaqueHIhi!"
    inspected = compile_template(source, Engine(libraries=LIBRARIES))
    first, second = (
        node.filter_expression.filters[0][0]
        for node in inspected.nodelist.get_nodes_by_type(VariableNode)
    )
    assert first.__name__ == "upper"
    assert second is shouting


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize(
    "body",
    [
        "{{ value|default }}",
        "{{ value|upper:1 }}",
        "{{ value|no_argument:1 }}",
        "{{ value|unknown }}",
        "{{ value|default }}{% unknown_tag %}",
        "{% hidden %}{{ value|default }}{% endhidden %}",
        "{% hidden %}{{ value|no_argument:1 }}{% endhidden %}",
        "{{ value| }}",
        "{{ value._private }}",
    ],
)
def test_compile_errors_match_django(body, debug):
    source = PREFIX + "<p>{{ fine }}</p>" + body
    errors = []
    for engine in engines(debug=debug):
        with pytest.raises(TemplateSyntaxError) as info:
            engine.from_string(source)
        errors.append(info.value)
    django, rusty = errors
    assert str(rusty) == str(django)
    if debug:
        assert rusty.template_debug["during"] == django.template_debug["during"]
        assert rusty.template_debug["line"] == django.template_debug["line"]


def test_variables_inside_hidden_bodies_render():
    source = PREFIX + "{% hidden %}{{ value|upper }}{% endhidden %}"
    django, rusty = (e.from_string(source) for e in engines())
    assert rusty.render(Context({"value": "x"})) == django.render(
        Context({"value": "x"})
    )
