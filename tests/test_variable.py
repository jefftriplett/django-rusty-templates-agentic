import functools
import types

import pytest
from inline_snapshot import snapshot


def test_render_variable(assert_render):
    template = "{{ foo }}"
    assert_render(template=template, context={"foo": 3}, expected="3")


def test_render_int(assert_render):
    template = "{{ 1 }}"
    assert_render(template=template, context={"1": 2}, expected="1")


def test_render_float(assert_render):
    template = "{{ 1.2 }}"
    assert_render(template=template, context={"1.2": 2}, expected="1.2")


def test_render_negative_int(assert_render):
    template = "{{ -1 }}"
    assert_render(template=template, context={"-1": 2}, expected="-1")


def test_render_negative_float(assert_render):
    template = "{{ -1.2 }}"
    assert_render(template=template, context={"-1.2": 2}, expected="-1.2")


def test_render_attribute_int(assert_render):
    template = "{{ foo.1 }}"
    assert_render(template=template, context={"foo": {1: 3}}, expected="3")


def test_render_variable_hyphen(assert_parse_error):
    template = "{{ foo-1 }}"
    django_message = snapshot("Could not parse the remainder: '-1' from 'foo-1'")
    rusty_message = snapshot("""\
  × Expected a valid variable name
   ╭────
 1 │ {{ foo-1 }}
   ·    ──┬──
   ·      ╰── here
   ╰────
""")
    assert_parse_error(
        template=template, django_message=django_message, rusty_message=rusty_message
    )


def test_render_attribute_negative_int(assert_parse_error):
    template = "{{ foo.-1 }}"
    django_message = snapshot("Could not parse the remainder: '-1' from 'foo.-1'")
    rusty_message = snapshot("""\
  × Expected a valid variable name
   ╭────
 1 │ {{ foo.-1 }}
   ·        ─┬
   ·         ╰── here
   ╰────
""")
    assert_parse_error(
        template=template, django_message=django_message, rusty_message=rusty_message
    )


def test_render_variable_remainder(assert_parse_error):
    template = "{{ a .b }}"
    django_message = snapshot("Could not parse the remainder: ' .b' from 'a .b'")
    rusty_message = snapshot("""\
  × Could not parse the remainder
   ╭────
 1 │ {{ a .b }}
   ·      ─┬
   ·       ╰── here
   ╰────
""")
    assert_parse_error(
        template=template, django_message=django_message, rusty_message=rusty_message
    )


def test_render_variable_remainder_before_filter(assert_parse_error):
    template = "{{ engines[0].name | length }}"
    django_message = snapshot(
        "Could not parse some characters: engines|[0].name| | length"
    )
    rusty_message = snapshot("""\
  × Could not parse the remainder
   ╭────
 1 │ {{ engines[0].name | length }}
   ·           ────┬───
   ·               ╰── here
   ╰────
""")
    assert_parse_error(
        template=template, django_message=django_message, rusty_message=rusty_message
    )


def test_render_float_exponent_sign(assert_parse_error):
    template = "{{ 1e+5 }}"
    django_message = snapshot("Could not parse the remainder: '+5' from '1e+5'")
    rusty_message = snapshot("""\
  × Could not parse the remainder
   ╭────
 1 │ {{ 1e+5 }}
   ·      ─┬
   ·       ╰── here
   ╰────
""")
    assert_parse_error(
        template=template, django_message=django_message, rusty_message=rusty_message
    )


def test_render_invalid_variable(assert_parse_error):
    template = "{{ & }}"
    django_message = snapshot("Could not parse the remainder: '&' from '&'")
    rusty_message = snapshot("""\
  × Expected a valid variable name
   ╭────
 1 │ {{ & }}
   ·    ┬
   ·    ╰── here
   ╰────
""")
    assert_parse_error(
        template=template, django_message=django_message, rusty_message=rusty_message
    )


def test_render_variable_callable(assert_render):
    template = "{{ foo }}"
    assert_render(template=template, context={"foo": lambda: 3}, expected="3")


def test_render_attribute_callable(assert_render):
    template = "{{ foo.bar }}"
    assert_render(template=template, context={"foo": {"bar": lambda: 3}}, expected="3")


class DoNotCall:
    do_not_call_in_templates = True
    attr = "attribute"

    def __call__(self):
        return "called"

    def __str__(self):
        return "not called"


class AltersData:
    def __init__(self):
        self.data = 0

    def increment(self):
        self.data += 1

    increment.alters_data = True


class Both:
    do_not_call_in_templates = True
    alters_data = True

    def __init__(self):
        self.data = 0

    def __call__(self):
        self.data += 1
        return "called"

    def __str__(self):
        return "not called"


def test_render_callable_do_not_call_in_templates(assert_render):
    template = "{{ do_not_call }}"
    context = {"do_not_call": DoNotCall()}
    assert_render(template=template, context=context, expected="not called")


def test_render_callable_do_not_call_in_templates_attribute(assert_render):
    template = "{{ do_not_call.attr }}"
    context = {"do_not_call": DoNotCall()}
    assert_render(template=template, context=context, expected="attribute")


def test_render_callable_attribute_alters_data(assert_render):
    template = "{{ foo.increment }}"
    mutable = AltersData()
    context = {"foo": mutable}
    assert_render(template=template, context=context, expected="")
    assert mutable.data == 0


def test_render_callable_variable_alters_data(assert_render):
    template = "{{ increment }}"
    mutable = AltersData()
    context = {"increment": mutable.increment}
    assert_render(template=template, context=context, expected="")
    assert mutable.data == 0


def test_do_not_call_and_alters_data(assert_render):
    template = "{{ foo.data }}"
    both = Both()
    context = {"foo": both}
    assert_render(template=template, context=context, expected="0")
    assert both.data == 0


class AttributeDict(dict):
    pass


class CallableTarget:
    def __init__(self, record):
        self.record = record

    def __call__(self, *args):
        return self.record()


def callable_value(kind, record):
    """Return a context value whose "method" is a callable of the given kind,
    and the object that carries that callable's template flags."""

    def function(*args):
        return record()

    if kind == "function":
        return {"method": function}, function
    if kind == "method":
        owner = type("Owner", (), {"method": function})()
        return owner, function
    if kind == "classmethod":
        owner = type("Owner", (), {"method": classmethod(function)})()
        return owner, function
    if kind == "function with dict subclass":
        function.__dict__ = AttributeDict()
        return {"method": function}, function
    if kind == "method of callable object":
        target = CallableTarget(record)
        return {"method": types.MethodType(target, object())}, target
    if kind == "callable object":
        target = CallableTarget(record)
        return {"method": target}, target
    if kind == "partial":
        target = functools.partial(function)
        return {"method": target}, target
    raise ValueError(kind)


CALLABLE_KINDS = [
    "function",
    "method",
    "classmethod",
    "function with dict subclass",
    "method of callable object",
    "callable object",
    "partial",
]


@pytest.mark.parametrize("kind", CALLABLE_KINDS)
@pytest.mark.parametrize(
    ("flags", "source", "expected", "calls"),
    [
        ({}, "{{ value.method }}", "called", 1),
        ({"alters_data": True}, "{{ value.method }}", "", 0),
        ({"alters_data": 0}, "{{ value.method }}", "called", 1),
        ({"do_not_call_in_templates": ""}, "{{ value.method }}", "called", 1),
        ({"do_not_call_in_templates": True}, "{{ value.method.label }}", "kept", 0),
        (
            {"do_not_call_in_templates": True, "alters_data": True},
            "{{ value.method.label }}",
            "kept",
            0,
        ),
    ],
)
def test_callable_template_flags(template_engine, kind, flags, source, expected, calls):
    record = []
    value, target = callable_value(kind, lambda: record.append(1) or "called")
    target.label = "kept"
    for name, flag in flags.items():
        setattr(target, name, flag)
    template = template_engine.from_string(source)
    assert template.render({"value": value}) == expected
    assert len(record) == calls


class BrokenFlag:
    def __bool__(self):
        raise ZeroDivisionError


@pytest.mark.parametrize("kind", CALLABLE_KINDS)
def test_callable_template_flag_errors(template_engine, kind):
    value, target = callable_value(kind, lambda: "called")
    target.alters_data = BrokenFlag()
    template = template_engine.from_string("{{ value.method }}")
    with pytest.raises(ZeroDivisionError):
        template.render({"value": value})
