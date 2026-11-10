"""A missing filter argument raises even when the filter wouldn't use it.

Django resolves a filter's arguments before it calls the filter, so
`{{ name|default:missing }}` raises `VariableDoesNotExist` although `name`
has a value.
"""

import pytest
from django.template import Context, Engine as DjangoEngine, VariableDoesNotExist

from django_rusty_templates import Engine

FILTERS = [
    "add",
    "center",
    "cut",
    "date",
    "default",
    "default_if_none",
    "divisibleby",
    "floatformat",
    "join",
    "ljust",
    "pluralize",
    "stringformat",
    "time",
    "truncatechars",
    "wordwrap",
    "yesno",
]
VALUES = ["absent", "text", "none", "empty", "number", "items"]


@pytest.mark.parametrize("engine_class", [DjangoEngine, Engine])
@pytest.mark.parametrize("value", VALUES)
@pytest.mark.parametrize("name", FILTERS)
def test_missing_argument_raises(engine_class, name, value):
    template = engine_class().from_string("{{ " + value + "|" + name + ":missing }}")
    context = {"text": "abc", "none": None, "empty": "", "number": 4, "items": [1]}
    with pytest.raises(VariableDoesNotExist):
        template.render(Context(context))
