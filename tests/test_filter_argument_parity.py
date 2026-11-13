"""Filter arguments from the context, the errors bad values raise, and
`string_if_invalid` (#414). Each template gives the same output, or the same
type of exception, as Django."""

import sys
from decimal import Decimal
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.template import TemplateSyntaxError
from django.utils.translation import gettext_lazy

from django_rusty_templates import Engine

MODULE = "rusty_filter_argument_parity_tags"

# `string_if_invalid` for each engine: unset, naming the variable, or plain.
INVALID = {"": "", "inv": "<%s>", "pln": "INV"}

CASES = [
    ("", "{{ s|wordwrap:w }}"),
    ("", "{{ s|wordwrap:w57 }}"),
    ("", "{{ s|wordwrap:d }}"),
    ("", "{{ s|wordwrap:ds }}"),
    ("", "{{ s|wordwrap:bo }}"),
    ("", "{{ s|wordwrap:none }}"),
    ("", "{{ s|wordwrap:inf }}"),
    ("", "{{ s|wordwrap:nan }}"),
    ("", "{{ s|wordwrap:big }}"),
    ("", "{{ s|wordwrap:b }}"),
    ("", "{{ s|wordwrap:neg }}"),
    ("", "{{ e|wordwrap:neg }}"),
    ("", "{{ s|wordwrap:-9223372036854775809 }}"),
    ("", "{{ s|wordwrap:0 }}"),
    ("", "{{ e|wordwrap:0 }}"),
    ("", "{{ s|center:d }}"),
    ("", "{{ s|center:w }}"),
    ("", "{{ s|center:w57 }}"),
    ("", "{{ s|ljust:d }}"),
    ("", "{{ s|rjust:w57 }}"),
    ("", "{{ s|ljust:bo }}"),
    ("", "{{ s|center:none }}"),
    ("", "{{ s|center:inf }}"),
    ("", "{{ s|truncatechars:d }}"),
    ("", "{{ s|truncatechars:w57 }}"),
    ("", "{{ s|truncatewords:d }}"),
    ("", "{{ s|truncatewords:w57 }}"),
    ("", "{{ s|truncatechars:f33 }}"),
    ("", "{{ l|slice:w }}"),
    ("", "{{ x|add:d }}"),
    ("", "{{ x|floatformat:d }}"),
    ("", "{{ x|floatformat:w57 }}"),
    ("", "{{ x|floatformat:w }}"),
    ("", "{{ l|get_digit:w }}"),
    ("", "{{ x|get_digit:d }}"),
    ("", "{{ x|divisibleby:d }}"),
    ("", "{{ x|divisibleby:w }}"),
    ("", "{{ x|pluralize:yn }}"),
    ("", "{{ v|yesno:yn }}"),
    ("", "{{ l|join:yn }}"),
    ("", "{{ s|cut:yn }}"),
    ("", "{{ e|default:yn }}"),
    ("", "{{ s|ljust:ds }}"),
    ("", "{{ s|stringformat:yn }}"),
    ("", "{{ inf|add:1 }}"),
    ("", "{{ nan|add:1 }}"),
    ("", "{{ lst|divisibleby:2 }}"),
    ("", "{{ decnan|divisibleby:2 }}"),
    ("", "{{ x|divisibleby:0 }}"),
    ("", "{{ s|divisibleby:0 }}"),
    ("", "{{ lst|divisibleby:0 }}"),
    ("", "{{ none|divisibleby:0 }}"),
    ("", '{% url "bio" "a" username="b" %}'),
    ("", "{% filter length %}x{% endfilter %}"),
    ("", "{% filter length|add:1 %}x{% endfilter %}"),
    ("", "{% filter first %}xy{% endfilter %}"),
    ("", '{% filter join:"-" %}xy{% endfilter %}'),
    ("", "{% lorem 1e999 %}"),
    ("", "{% lorem inf w %}"),
    ("", "{% lorem w w %}"),
    ("", "{% lorem d w %}"),
    ("", "{% load p %}{% inc_none %}"),
    ("", "{% load p %}{{ s|ident:1 }}"),
    ("", "{% load p %}{{ s|typ2 }}"),
    ("", "{% load p %}{{ s|opt }}"),
    ("", "{% load p %}{{ s|opt:1 }}"),
    ("", '{% load p %}{{ s|typ2:"tr" }}'),
    ("", "{% load p %}{{ s|typ2:1 }}"),
    ("", "{% load p %}{{ s|typ2:1.5 }}"),
    ("", "{% load p %}{{ s|typ2:yn }}"),
    ("", "{{ s|upper:1 }}"),
    ("", "{{ s|default }}"),
    ("", "{{ s|cut }}"),
    ("", "{{ s|length:1 }}"),
    ("", "{{ s|date:1:2 }}"),
    ("inv", "{{ missing|default:'x'|upper }}"),
    ("inv", "{{ missing|upper }}"),
    ("inv", "{{ missing }}"),
    ("inv", "{{ missing|default:'x' }}"),
    ("inv", "{{ missing|default_if_none:'x' }}"),
    ("inv", "{{ a.needs }}"),
    ("inv", "{{ a.alt }}"),
    ("inv", "{{ a.needs|upper }}"),
    ("inv", "{{ a.missing.deeper }}"),
    ("inv", "{% if missing %}y{% else %}n{% endif %}"),
    ("inv", "{% firstof missing 'x' %}"),
    ("inv", "{% with y=missing %}[{{ y }}]{% endwith %}"),
    ("inv", "{% for x in missing %}{{ x }}{% empty %}e{% endfor %}"),
    ("", '{% if a.needs == "" %}y{% else %}n{% endif %}'),
    ("", "{% if a.needs is None %}y{% else %}n{% endif %}"),
    ("", '{% if a.alt == "" %}y{% else %}n{% endif %}'),
    ("", "{% if a.needs %}y{% else %}n{% endif %}"),
    ("", '{% if a.missing == "" %}y{% else %}n{% endif %}'),
    ("", "{% if a.missing is None %}y{% else %}n{% endif %}"),
    ("", "{% firstof a.needs 'x' %}"),
    ("", '{{ a.needs|default:"d" }}'),
    ("", '{{ a.needs|default_if_none:"d" }}'),
    ("", "{{ a.needs|length }}"),
    ("", "{% with y=a.needs %}[{{ y }}]{% endwith %}"),
    ("pln", "{{ missing|default:'x'|upper }}"),
    ("pln", "{{ missing|upper }}"),
    ("pln", "{{ missing }}"),
    ("pln", "{{ missing|default:'x' }}"),
    ("pln", "{{ missing|default_if_none:'x' }}"),
    ("pln", "{{ a.needs }}"),
    ("pln", "{{ a.alt }}"),
    ("pln", "{{ a.needs|upper }}"),
    ("pln", "{{ a.missing.deeper }}"),
    ("pln", "{% if missing %}y{% else %}n{% endif %}"),
    ("pln", "{% firstof missing 'x' %}"),
    ("pln", "{% with y=missing %}[{{ y }}]{% endwith %}"),
    ("pln", "{% for x in missing %}{{ x }}{% empty %}e{% endfor %}"),
    ("pln", "{{ missing|default:'x'|upper|lower }}"),
    ("inv", "{{ missing|default:'x'|upper|lower }}"),
    ("pln", "{{ a.needs|default:'x' }}"),
    ("inv", "{{ a.needs.x }}"),
    ("", "{{ a.needs.x }}[{{ a.needs.upper }}]"),
    ("", "{% if a.needs.x %}y{% else %}n{% endif %}"),
    ("pln", "{% if a.needs %}y{% else %}n{% endif %}"),
    ("pln", "{{ a.alt|upper }}"),
]


@pytest.fixture(autouse=True)
def library(monkeypatch):
    module = ModuleType(MODULE)
    module.register = Library()

    @module.register.filter
    def ident(value):
        return value

    @module.register.filter
    def typ2(value, argument):
        return type(argument).__name__

    @module.register.filter
    def opt(value, argument=None):
        return f"{value}{argument}"

    @module.register.inclusion_tag("inc.html")
    def inc_none():
        return None

    monkeypatch.setitem(sys.modules, MODULE, module)


class Methods:
    def needs(self, argument):
        return "n"

    def alt(self):
        return "alt"

    alt.alters_data = True


def values():
    return {
        "s": "hello world foo",
        "e": "",
        "w": 5.0,
        "w57": 5.7,
        "d": Decimal("5"),
        "ds": "5",
        "neg": "-3",
        "v": True,
        "yn": gettext_lazy("yes,no,maybe"),
        "l": [10, 20, 30],
        "inf": float("inf"),
        "nan": float("nan"),
        "lst": [1],
        "decnan": Decimal("nan"),
        "x": 4,
        "a": Methods(),
        "t": True,
        "b": b"5",
        "bo": True,
        "none": None,
        "f33": 3.3,
        "big": 10**30,
    }


def options(invalid):
    return {
        "libraries": {"p": MODULE},
        "loaders": [
            ("django.template.loaders.locmem.Loader", {"inc.html": "[{{ x }}]"})
        ],
        "string_if_invalid": INVALID[invalid],
    }


def outcome(render):
    try:
        return render()
    except Exception as error:
        return type(error)


@pytest.mark.parametrize(("invalid", "source"), CASES)
def test_matches_django(invalid, source):
    expected = outcome(
        lambda: (
            DjangoEngine(**options(invalid))
            .from_string(source)
            .render(Context(values()))
        )
    )
    assert (
        outcome(lambda: Engine(**options(invalid)).from_string(source).render(values()))
        == expected
    )


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("{{ s|wordwrap:w }}", "hello\nworld\nfoo"),
        ("{{ s|wordwrap:d }}", "hello\nworld\nfoo"),
        ("{{ s|center:d }}", "hello world foo"),
        ("{{ v|yesno:yn }}", "yes"),
        ("{% load p %}{% inc_none %}", "[]"),
        ('{% if a.needs == "" %}y{% else %}n{% endif %}', "y"),
        ("{% if a.needs is None %}y{% else %}n{% endif %}", "n"),
    ],
)
def test_renders(source, expected):
    assert Engine(**options("")).from_string(source).render(values()) == expected


@pytest.mark.parametrize(
    ("invalid", "source", "expected"),
    [
        ("inv", "{{ missing|default:'x'|upper }}", "&lt;missing&gt;"),
        ("pln", "{{ missing|default:'x'|upper }}", "INV"),
        ("inv", "{{ a.needs }}", "&lt;%s&gt;"),
        ("inv", "{{ a.needs|upper }}", "&lt;%S&gt;"),
        ("pln", "{{ a.alt|lower }}", "inv"),
    ],
)
def test_string_if_invalid(invalid, source, expected):
    assert Engine(**options(invalid)).from_string(source).render(values()) == expected


@pytest.mark.parametrize(
    ("source", "error"),
    [
        ("{{ s|wordwrap:none }}", TypeError),
        ("{{ s|wordwrap:nan }}", ValueError),
        ("{{ s|wordwrap:-9223372036854775809 }}", ValueError),
        ("{{ inf|add:1 }}", OverflowError),
        ("{{ x|divisibleby:0 }}", ZeroDivisionError),
        ("{{ lst|divisibleby:0 }}", TypeError),
        ("{% filter length %}x{% endfilter %}", TypeError),
        ("{% lorem 1e999 %}", OverflowError),
    ],
)
def test_raises_at_render(source, error):
    template = Engine(**options("")).from_string(source)
    with pytest.raises(error):
        template.render(values())


@pytest.mark.parametrize(
    ("source", "message"),
    [
        ("{% load p %}{{ s|ident:1 }}", "ident requires 1 arguments, 2 provided"),
        ("{% load p %}{{ s|typ2 }}", "typ2 requires 2 arguments, 1 provided"),
    ],
)
def test_filter_arity_is_checked_when_compiling(source, message):
    with pytest.raises(TemplateSyntaxError) as info:
        Engine(**options("")).from_string(source)
    assert message in str(info.value)
