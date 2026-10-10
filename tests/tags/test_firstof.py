import pytest
from inline_snapshot import snapshot


@pytest.mark.parametrize(
    "template, context, expected",
    [
        ("{% firstof a b c %}", {"a": 0, "c": 0, "b": 0}, ""),
        ("{% firstof a b c %}", {"a": 1, "c": 0, "b": 0}, "1"),
        ("{% firstof a b c %}", {"a": 0, "c": 0, "b": 2}, "2"),
        ("{% firstof a b c %}", {"a": 0, "c": 3, "b": 0}, "3"),
        ("{% firstof a b c %}", {"a": 1, "c": 3, "b": 2}, "1"),
        ("{% firstof a b c %}", {"c": 3, "b": 0}, "3"),
        ('{% firstof a b "c" %}', {"a": 0}, "c"),
        ('{% firstof a b "c and d" %}', {"a": 0, "b": 0}, "c and d"),
        ("{% firstof a %}", {"a": "<"}, "&lt;"),
        ("{% firstof a b %}", {"a": "<", "b": ">"}, "&lt;"),
        ("{% firstof a b %}", {"a": "", "b": ">"}, "&gt;"),
        ("{% autoescape off %}{% firstof a %}{% endautoescape %}", {"a": "<"}, "<"),
        ("{% firstof a|safe b %}", {"a": "<"}, "<"),
        ("{% firstof a|default:'fallback' %}", {"a": ""}, "fallback"),
        ("{% firstof a b %}", {"a": [1, 2], "b": 3}, "[1, 2]"),
        ("{% firstof a b %}", {"a": [], "b": 3}, "3"),
    ],
)
def test_firstof_render(assert_render, template, context, expected):
    assert_render(
        template=template,
        context=context,
        expected=expected,
    )


def test_firstof_asvar(assert_render):
    assert_render(
        template="{% firstof a b c as myvar %}{{ myvar }}",
        context={"a": 0, "b": 2, "c": 3},
        expected="2",
    )


def test_all_false_arguments_asvar(assert_render):
    assert_render(
        template="{% firstof a b c as myvar %}{{ myvar }}",
        context={"a": 0, "b": 0, "c": 0},
        expected="",
    )


def test_firstof_as_variable_name(assert_render):
    # 'as' can be a variable name if not used as the keyword
    assert_render(
        template="{% firstof as b %}",
        context={"as": "foo", "b": "bar"},
        expected="",
    )


def test_firstof_missing_argument_error(assert_parse_error):
    assert_parse_error(
        template="{% firstof %}",
        django_message="'firstof' statement requires at least one argument",
        rusty_message=snapshot("""\
  × Expected an argument
   ╭────
 1 │ {% firstof %}
   ·           ▲
   ·           ╰── here
   ╰────
"""),
    )


def test_firstof_incomplete_kwarg_error(assert_parse_error):
    assert_parse_error(
        template="{% firstof a= %}",
        django_message="Could not parse the remainder: '=' from 'a='",
        rusty_message=snapshot("""\
  × Incomplete keyword argument
   ╭────
 1 │ {% firstof a= %}
   ·            ─┬
   ·             ╰── here
   ╰────
"""),
    )


def test_firstof_invalid_as_usage_error(assert_render):
    assert_render(
        template="{% firstof a as %}",
        context={"a": 1},
        expected="1",
    )


def test_firstof_invalid_as_variable_error(assert_render):
    assert_render(
        template="{% firstof a as 123 %}",
        context={"a": 0, "as": "foo"},
        expected="",
    )


def test_firstof_too_many_arguments_after_as(assert_render):
    assert_render(
        template="{% firstof a as b c %}",
        context={"a": 0, "as": "foo", "b": "bar", "c": "baz"},
        expected="foo",
    )


def test_firstof_returns_first_truthy_value(assert_render):
    assert_render(
        template="{% firstof a b %}",
        context={"a": 0, "b": 2},
        expected="2",
    )


def test_firstof_sets_asvar_when_truthy(assert_render):
    assert_render(
        template="{% firstof a b as out %}{{ out }}",
        context={"a": 0, "b": 3},
        expected="3",
    )


def test_firstof_sets_asvar_empty_when_all_false(assert_render):
    assert_render(
        template="{% firstof a b as out %}{{ out }}",
        context={"a": 0, "b": 0},
        expected="",
    )


def test_firstof_numeric_literals(assert_render):
    assert_render(
        template="{% firstof 0 1 %}",
        context={},
        expected="1",
    )


def test_firstof_existing_false_then_truthy(assert_render):
    assert_render(
        template="{% firstof a b %}",
        context={"a": "", "b": "ok"},
        expected="ok",
    )


@pytest.mark.parametrize("value, expected", [(2, "2"), ("<", "&lt;")])
def test_firstof_asvar_stores_rendered_string(template_engine, value, expected):
    from django.template import Context

    context = Context({"value": value})
    template = template_engine.engine.from_string(
        "{% firstof value as chosen %}{{ chosen }}"
    )
    assert template.render(context) == expected
    assert context["chosen"] == expected
    assert isinstance(context["chosen"], str)
