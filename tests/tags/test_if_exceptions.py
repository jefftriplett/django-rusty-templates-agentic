"""`{% if %}` raises, swallows and short-circuits as Django's smartif does.

Django lets an exception escape a bare `{% if x %}` (other than a missing
variable), catches every `Exception` inside an operator, evaluates the right
side of `and`/`or` only when needed, and tests the truth of the value
`and`/`or` return outside the operator.
"""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library

from django_rusty_templates import Engine


class Interrupt(BaseException):
    """Not an `Exception`, so Django's operators don't catch it."""


class Silent(Exception):
    silent_variable_failure = True


def make_context(log):
    class Boom:
        def __bool__(self):
            log.append("boom.__bool__")
            raise RuntimeError("bool")

    class BaseBoom:
        def __bool__(self):
            raise Interrupt

    class BadLen:
        def __len__(self):
            raise RuntimeError("len")

    class BadEq:
        def __eq__(self, other):
            log.append("badeq.__eq__")
            raise RuntimeError("eq")

        __hash__ = None

    class BadNe:
        def __ne__(self, other):
            raise RuntimeError("ne")

    class OnlyNe:
        """`!=` must use `__ne__`, not the opposite of `__eq__`."""

        def __eq__(self, other):
            return False

        def __ne__(self, other):
            return False

        __hash__ = None

    class BadOrder:
        def __lt__(self, other):
            raise RuntimeError("lt")

        __gt__ = __le__ = __ge__ = __lt__

    class BadContains:
        def __contains__(self, item):
            raise RuntimeError("contains")

    class Counted:
        def __init__(self, name, truth):
            self.name, self.truth = name, truth

        def __bool__(self):
            log.append(f"{self.name}.__bool__")
            return self.truth

    class Holder:
        @property
        def prop(self):
            log.append("prop")
            raise RuntimeError("prop")

        @property
        def silent(self):
            raise Silent

        def method(self):
            log.append("method")
            return True

    def side():
        log.append("side")
        return True

    def raising():
        log.append("raising")
        raise RuntimeError("call")

    return {
        "t": True,
        "f": False,
        "one": 1,
        "text": "abc",
        "items": [1, 2],
        "boom": Boom(),
        "baseboom": BaseBoom(),
        "badlen": BadLen(),
        "badeq": BadEq(),
        "badne": BadNe(),
        "onlyne": OnlyNe(),
        "badorder": BadOrder(),
        "badcontains": BadContains(),
        "yes": Counted("yes", True),
        "no": Counted("no", False),
        "o": Holder(),
        "side": side,
        "raising": raising,
    }


CONDITIONS = [
    # A bare condition: only a missing variable is ignored.
    "t",
    "f",
    "missing",
    "missing.attr",
    "o.missing",
    "boom",
    "badlen",
    "baseboom",
    "o.prop",
    "o.silent",
    "o.method",
    "raising",
    "text|default:missing",
    "text|explode",
    "missing|explode",
    "yes",
    "no",
    # `not` catches what its operand raises.
    "not boom",
    "not o.prop",
    "not missing",
    "not yes",
    "not baseboom",
    "not text|explode",
    # `and` and `or` short-circuit and return an operand.
    "f and side",
    "t and side",
    "t or side",
    "f or side",
    "no and yes",
    "yes and no",
    "yes or no",
    "no or yes",
    "yes and yes and no",
    "no or no or yes",
    "boom and t",
    "t and boom",
    "f and boom",
    "boom or t",
    "f or boom",
    "t or boom",
    "o.prop and t",
    "t and o.prop",
    "o.prop or t",
    "f or o.prop",
    "t and boom and t",
    "t and boom or t",
    "f or boom or t",
    "t and raising",
    "f or raising",
    "missing and side",
    "missing or side",
    "baseboom and t",
    "not f and boom",
    "not boom and t",
    "not boom or t",
    # Comparisons catch what the operands and the comparison raise.
    "badeq == one",
    "one == badeq",
    "badeq != one",
    "one != badeq",
    "badne != one",
    "badne == one",
    "onlyne != one",
    "onlyne == one",
    "badorder < one",
    "badorder > one",
    "badorder <= one",
    "badorder >= one",
    "one < text",
    "one in badcontains",
    "one not in badcontains",
    "one in one",
    "one not in one",
    "one in missing",
    "one not in missing",
    "missing in items",
    "missing not in items",
    "o.prop == one",
    "o.prop != one",
    "one == o.prop",
    "o.prop in items",
    "o.prop is None",
    "o.prop is not None",
    "raising == one",
    "text|explode == one",
    "missing == None",
    "missing != None",
    "missing is None",
    "missing is not None",
    "missing == missing",
    "missing < one",
    "boom == boom",
    "boom is boom",
    "boom is not boom",
    "badeq == one and side",
    "badeq != one or side",
    "not badeq == one",
    "not one in badcontains",
    "not one not in badcontains",
    "t == not f",
    "not f == t",
    "not f is t",
    "not f is not f",
    "t is True",
    "f is False",
    "t is not True",
    "yes == t",
    "yes and one == one",
    "no or one == one",
    "one == one and boom",
    "one != one or boom",
]


@pytest.fixture
def libraries(monkeypatch):
    module = ModuleType("if_exception_filters")
    module.register = Library()

    @module.register.filter
    def explode(value):
        raise RuntimeError("filter")

    monkeypatch.setitem(sys.modules, module.__name__, module)
    return {"explode": module.__name__}


def outcome(engine_class, libraries, source):
    log = []
    template = engine_class(libraries=libraries).from_string(source)
    try:
        result = template.render(Context(make_context(log)))
    except BaseException as error:  # noqa: BLE001 - `Interrupt` isn't an Exception
        result = type(error).__name__
    return result, log


@pytest.mark.parametrize("condition", CONDITIONS)
def test_if_condition(condition, libraries):
    source = "{% load explode %}{% if " + condition + " %}yes{% else %}no{% endif %}"
    expected = outcome(DjangoEngine, libraries, source)
    assert outcome(Engine, libraries, source) == expected


@pytest.mark.parametrize(
    "source",
    [
        "{% if boom %}a{% elif t %}b{% endif %}",
        "{% if f %}a{% elif boom %}b{% else %}c{% endif %}",
        "{% if f %}a{% elif not boom %}b{% else %}c{% endif %}",
        "{% if t %}a{% elif boom %}b{% endif %}",
        "{% if no %}a{% elif yes %}b{% elif side %}c{% endif %}",
        "{% for x in items %}{% if x == one or side %}{{ x }}{% endif %}{% endfor %}",
    ],
)
def test_if_branches(source, libraries):
    expected = outcome(DjangoEngine, libraries, source)
    assert outcome(Engine, libraries, source) == expected
