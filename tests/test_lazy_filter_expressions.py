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
from django.template.defaulttags import ForNode, IfNode, do_if

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


override_library = Library()


@override_library.tag("if")
def wrapped_if(parser, token):
    return do_if(parser, token)


module = ModuleType("lazy_filter_expressions")
module.register = library
sys.modules[module.__name__] = module
shout_module = ModuleType("lazy_filter_expressions_shout")
shout_module.register = shout_library
sys.modules[shout_module.__name__] = shout_module
override_module = ModuleType("lazy_filter_expressions_override")
override_module.register = override_library
sys.modules[override_module.__name__] = override_module
LIBRARIES = {
    "lazy": module.__name__,
    "shout": shout_module.__name__,
    "override": override_module.__name__,
}
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
        # Arguments of {% for %}, {% if %} and {% elif %}.
        "{% for x in value|default %}{% endfor %}",
        "{% for x in value|upper:1 %}{% endfor %}",
        "{% for x in value._private %}{% endfor %}",
        "{% if value|default %}{% endif %}",
        "{% if value| %}{% endif %}",
        "{% if a %}{% elif value|no_argument:1 %}{% endif %}",
        "{% if a %}{% elif b == value|default %}{% endif %}",
        "{% if value|default and %}{% endif %}",
        "{% if value|default %}{% for x in y %}{% endif %}",
        "{% for x in value|default %}{{ y|unknown }}{% endfor %}",
        # Spans Rust doesn't compile, so Django checks the arguments.
        "{% for x in a %}{% if value|default %}{% endif %}"
        "{{ forloop.parentloop }}{% endfor %}",
        "{% for x in value|default %}{{ forloop.parentloop }}{% endfor %}",
        "{% for x, y in value|upper:1 %}{% endfor %}",
        "{% hidden %}{% if value|default %}{% endif %}{% endhidden %}",
        "{% hidden %}{% for x in value|upper:1 %}{% endfor %}{% endhidden %}",
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


def test_control_flow_arguments_are_lazy_and_inspect_like_django():
    source = (
        PREFIX + "{% for x in items|slice:':2' reversed %}{% if x > 1 and x|add:1 %}"
        "a{% elif x == 'one' %}b{% else %}c{% endif %}{% endfor %}"
    )
    django = DjangoEngine(libraries=LIBRARIES).from_string(source)
    rusty = compile_template(source, Engine(libraries=LIBRARIES))

    def arguments(template):
        (loop,) = template.nodelist.get_nodes_by_type(ForNode)
        (condition,) = template.nodelist.get_nodes_by_type(IfNode)
        literals = []
        pending = [c for c, _ in condition.conditions_nodelists if c is not None]
        while pending:
            operator = pending.pop()
            if operator.id == "literal":
                literals.append(operator.value)
            else:
                pending.extend(
                    part
                    for part in (operator.first, operator.second)
                    if part is not None
                )
        return [loop.sequence, *literals]

    lazy = arguments(rusty)
    assert all(type(e) is LazyFilterExpression for e in lazy)

    def filters(expression):
        return [
            (function, [(lookup, str(argument)) for lookup, argument in args])
            for function, args in expression.filters
        ]

    for expression, eager in zip(lazy, arguments(django), strict=True):
        assert str(expression) == str(eager)
        assert str(expression.var) == str(eager.var)
        assert filters(expression) == filters(eager)
    for items in ([3, 2, 1], ["one", 0], []):
        context = {"items": items}
        assert rusty.render(Context(context)) == django.render(Context(context))


@pytest.mark.parametrize(
    "body",
    [
        "{% for x in items|slice:':2' %}{% if x|add:1 > 2 %}{{ x }}{% endif %}"
        "{% empty %}none{% endfor %}",
        "{% for x in items %}{% for y in items|slice:':1' %}"
        "{{ forloop.parentloop.counter }}{% if y %}{{ y }}{% endif %}"
        "{% endfor %}{% endfor %}",
        "{% hidden %}{% for x in items|first|make_list %}{% if x %}{{ x }}"
        "{% endif %}{% endfor %}{% endhidden %}",
        "{% if items|length > 1 %}many{% elif items %}one{% endif %}",
    ],
)
def test_control_flow_arguments_render_like_django(body):
    source = PREFIX + body
    django, rusty = (e.from_string(source) for e in engines())
    for items in ([3, 2, 1], ["one"], []):
        context = {"items": items}
        assert rusty.render(Context(context)) == django.render(Context(context))


def test_replaced_if_compiles_its_arguments_eagerly():
    source = PREFIX + "{% load override %}{% if value|default:1 %}x{% endif %}"
    rusty = compile_template(source, Engine(libraries=LIBRARIES))
    (condition,) = rusty.nodelist.get_nodes_by_type(IfNode)
    assert type(condition.conditions_nodelists[0][0].value) is FilterExpression
