"""Rare differences from Django (#424): a variable named `forloop` inside a
loop, digits of other scripts, and a translated literal given to a custom
filter. Each template renders the same through Django."""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.utils.functional import Promise

from django_rusty_templates import Engine

MODULE = "rusty_rare_differences_tags"


def build_library():
    register = Library()

    @register.filter
    def describe(value, argument):
        lazy = isinstance(argument, Promise)
        return f"{value}:{argument}:{'lazy' if lazy else type(argument).__name__}"

    @register.simple_tag(takes_context=True)
    def set_forloop(context, value):
        context["forloop"] = value
        return ""

    return register


@pytest.fixture(autouse=True)
def library(monkeypatch):
    module = ModuleType(MODULE)
    module.register = build_library()
    monkeypatch.setitem(sys.modules, MODULE, module)


CONTEXT = {
    "items": [10, 20, 30],
    "pairs": [(1, 2), (3, 4)],
    "long": [10, 20, 30, 1, 1, 1, 1, 1, 1, 1, 99],
    "names": {"1": "ascii", "١": "arabic"},
    "empty": "",
    "one": 1,
}

FORLOOP = [
    # The loop variable replaces the loop object.
    "{% for forloop in items %}{{ forloop }}{% endfor %}",
    "{% for forloop in items %}{{ forloop.counter }}|{{ forloop.real }}{% endfor %}",
    "{% for forloop in items %}{% if forloop.first %}F{% endif %}"
    "{% if forloop == 20 %}T{% endif %}{% endfor %}",
    "{% for forloop, other in pairs %}{{ forloop }}{{ other }}{% endfor %}",
    "{% for forloop in items %}{{ forloop|add:forloop }}{% endfor %}",
    # So does anything else that binds the name in the body.
    "{% for x in items %}{% with forloop=5 %}{{ forloop }}{{ forloop.counter }}"
    "{% endwith %}{{ forloop.counter }}{% endfor %}",
    "{% for x in items %}{% firstof x as forloop %}{{ forloop }}"
    "{{ forloop.counter }}{% endfor %}",
    "{% for x in items %}{{ forloop.counter }}{% if x == 20 %}"
    "{% firstof x as forloop %}{% endif %}{% endfor %}",
    "{% for x in items %}{% set_forloop 9 %}{{ forloop }}{% endfor %}",
    # A loop inside starts a new loop object, with the variable as its parent.
    "{% for forloop in items %}{% for y in items %}{{ forloop.counter }}"
    "{{ forloop.parentloop }}{% endfor %}{% endfor %}",
    "{% for forloop in pairs %}{% for y in forloop %}{{ forloop.parentloop.0 }}"
    "{% endfor %}{% endfor %}",
    "{% for x in items %}{% for forloop in items %}{{ forloop }}"
    "{{ forloop.parentloop.counter }}{% endfor %}{{ forloop.counter }}{% endfor %}",
    # A binding from outside the loop is hidden by it, and is its parent.
    "{% with forloop=7 %}{% for x in items %}{{ forloop.counter }}{% endfor %}"
    "{{ forloop }}{% endwith %}",
    "{% with forloop=7 %}{% for x in items %}{{ forloop.parentloop }}{% endfor %}"
    "{% endwith %}",
]

DIGITS = [
    "{{ ١٢ }}",
    "{{ -١٢ }}",
    "{{ ١.٥ }}",
    "{{ ٣e٢ }}",
    "{{ １２ }}",
    "{{ ١x }}",
    "{{ items.١ }}",
    "{{ items.٠ }}",
    "{{ items.١٢ }}",
    "{{ long.1_0 }}{{ long.0_1 }}",
    "{{ names.١ }}{{ names.1 }}",
    "{{ empty|default:١٢ }}",
    "{{ one|add:１２ }}",
    "{% if ١٢ == 12 %}yes{% endif %}",
    "{% widthratio ٥ ١٠ ١٠٠ %}",
]

TRANSLATED = [
    '{{ one|describe:_("text") }}',
    "{{ one|describe:_('a < b') }}",
    '{{ one|describe:"text" }}',
    "{{ one|describe:one }}",
]


def render_both(source):
    source = f"{{% load {MODULE} %}}{source}"
    libraries = {MODULE: MODULE}
    django = DjangoEngine(libraries=libraries).from_string(source)
    rusty = Engine(libraries=libraries).from_string(source)
    return django.render(Context(dict(CONTEXT))), rusty.render(Context(dict(CONTEXT)))


@pytest.mark.parametrize("source", FORLOOP + DIGITS + TRANSLATED)
def test_renders_as_django(source):
    expected, actual = render_both(source)
    assert actual == expected


def test_loop_variable_named_forloop_prints_items():
    _, actual = render_both("{% for forloop in items %}{{ forloop }},{% endfor %}")
    assert actual == "10,20,30,"


def test_arabic_digits_are_a_number():
    _, actual = render_both("{{ ١٢|add:1 }}{{ items.١ }}")
    assert actual == "1320"


def test_custom_filter_gets_a_lazy_translation():
    _, actual = render_both('{{ one|describe:_("text") }}')
    assert actual == "1:text:lazy"
