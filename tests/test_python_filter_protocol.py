"""Filters written in Python see the same values and flags as in Django."""

import datetime
import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.test import override_settings
from django.utils.safestring import SafeData, mark_safe

from django_rusty_templates import Engine

library = Library()


@library.filter
def safety(value, arg=None):
    return f"{type(value).__name__}:{isinstance(value, SafeData)}/{isinstance(arg, SafeData)}"


@library.filter
def show(value, arg="-"):
    return f"[{value!r}|{arg!r}]"


@library.filter
def is_none(value):
    return value is None


@library.filter(needs_autoescape=True)
def autoescape_flag(value, autoescape=True):
    return f"autoescape={autoescape}"


@library.filter(is_safe=True)
def exclaim(value):
    return value + "!"


@library.filter(expects_localtime=True)
def hour(value):
    return value.hour


module = ModuleType("python_filter_protocol")
module.register = library
sys.modules[module.__name__] = module
LIBRARIES = {"protocol": module.__name__}


def render_both(source, values, **options):
    source = "{% load protocol %}" + source
    return [
        cls(libraries=LIBRARIES, **options).from_string(source).render(Context(values))
        for cls in (DjangoEngine, Engine)
    ]


LITERALS = [
    '{{ "<b>"|safety:"<i>" }}',
    '{{ x|safety:"<i>" }}',
    '{{ x|default:"<b>"|safety }}',
    '{{ "<b>"|escape }}',
    '{% with y="<b>" %}{% autoescape on %}{{ y }}{% endautoescape %}{% endwith %}',
    '{% with y=_("<b>") %}{% autoescape on %}{{ y }}{% endautoescape %}{% endwith %}',
    '{% firstof "<b>" as z %}{% autoescape on %}{{ z }}{% endautoescape %}',
    '{% cycle "<b>" "<i>" as c silent %}{% autoescape on %}{{ c }}{% endautoescape %}',
    '{{ x|default:"<b>"|force_escape }}',
    '{{ items|join:"<br>" }}',
]


@pytest.mark.parametrize("autoescape", ["on", "off"])
@pytest.mark.parametrize("source", LITERALS)
def test_string_literals_are_safe(source, autoescape):
    source = f"{{% autoescape {autoescape} %}}{source}{{% endautoescape %}}"
    django, rusty = render_both(source, {"x": "", "items": ["<a>", "<b>"]})
    assert rusty == django


@pytest.mark.parametrize(
    "source,expected",
    [
        ("{{ s|autoescape_flag }}", "autoescape=True"),
        (
            "{% autoescape off %}{{ s|autoescape_flag }}{% endautoescape %}",
            "autoescape=False",
        ),
        ("{{ safe|exclaim }}", "<i>!"),
        ("{{ s|exclaim }}", "&lt;b&gt;!"),
        ("{{ d|hour }}", "6"),
    ],
)
@override_settings(USE_TZ=True, TIME_ZONE="America/Chicago")
def test_filter_flags(source, expected):
    values = {
        "s": "<b>",
        "safe": mark_safe("<i>"),
        "d": datetime.datetime(2026, 1, 1, 12, tzinfo=datetime.timezone.utc),
    }
    assert render_both(source, values) == [expected, expected]


@pytest.mark.parametrize("string_if_invalid", ["", "INVALID"])
@pytest.mark.parametrize(
    "source",
    [
        "{{ missing|show }}",
        "{{ missing.attribute|show }}",
        "{% if missing|is_none %}yes{% else %}no{% endif %}",
        "{% for x in missing|show %}{{ x }}{% endfor %}",
        "{{ 1|show:2.5 }}",
    ],
)
def test_missing_values(source, string_if_invalid):
    django, rusty = render_both(source, {}, string_if_invalid=string_if_invalid)
    assert rusty == django
