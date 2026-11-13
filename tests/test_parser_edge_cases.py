"""Templates at the edges of the syntax, which Rusty used to accept where
Django raises, or reject where Django renders (#413). Each one gives the same
output, or the same type of exception, as Django."""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.template import TemplateSyntaxError

from django_rusty_templates import Engine

MODULE = "rusty_parser_edge_case_tags"

SOURCES = [
    "{% cycle 'a'|upper 'b' %}",
    "{% cycle 'a' 'b'|upper %}",
    '{% cycle s|upper "b"|upper as c %}{{ c }}',
    "{% cycle 'a b'|upper 'b' %}",
    "{% cycle 1|add:1 2 %}",
    "{{ es|default:1_0 }}",
    "{{ 1_0 }}",
    "{{ 1_0.5 }}",
    "{{ 1_000 }}",
    "{{ _1 }}",
    "{{ 1_ }}",
    "{{ 1__0 }}",
    "{{ d.1_0 }}",
    "{{ s. }}",
    "{{ .s }}",
    "{{ s..x }}",
    "{{ . }}",
    "{{ .. }}",
    "{{ l.0. }}",
    "{{ s|default:. }}",
    "{% cycle n . %}",
    "{% if s. %}y{% else %}n{% endif %}",
    "{% if . %}y{% else %}n{% endif %}",
    "{{ 1e309 }}",
    "{{ -1e309 }}",
    "{{ es|default:1e309 }}",
    "{{ 1e5 }}",
    "{{ 1E5 }}",
    "{{ 1.e3 }}",
    "{{ s|upper| }}",
    "{{ s| }}",
    "{{ s|upper|lower| }}",
    '{{ s|default:"x"| }}',
    "{% if s|upper| %}y{% endif %}",
    "{% if a %}{% endif x %}",
    "{% if a %}{% else x %}{% endif %}",
    "{% if a %}{% endif a %}",
    "{% for x in l %}{% empty x %}{% endfor %}",
    "{% for x in l %}{% endfor x %}",
    "{% for x in l %}{% endfor l %}",
    "{% with a=1 as b %}{{ a }}{{ b }}{% endwith %}",
    "{% with a=1 %}{% endwith a %}",
    "{% with 1 as b %}{{ b }}{% endwith %}",
    "{% with 1 as b c=2 %}{{ b }}{% endwith %}",
    "{% for x in reversed %}{{ x }}{% endfor %}",
    "{% for x in l reversed %}{{ x }}{% endfor %}",
    "{% for x in reversed reversed %}{{ x }}{% endfor %}",
    "{{ - }}",
    "{{ -s }}",
    "{{ + }}",
    "{{ -1 }}",
    "{{ +1 }}",
    "{{ -.5 }}",
    "{{ s|default:- }}",
    "{% load p %}{% st =1 %}",
    "{% load p %}{% st a=1 %}",
    "{% load p %}{% st = %}",
    "{% load p %}{% st 1=1 %}",
    "{% block a %}{% endblock b %}",
    "{% block a %}{% endblock a %}",
    "{% block a %}{% endblock a b %}",
    "{% comment %}{% endcomment x %}",
    "{% spaceless %}{% endspaceless x %}",
    "{% autoescape on %}{% endautoescape x %}",
    "{% filter upper %}a{% endfilter x %}",
    "{% verbatim %}{% endverbatim x %}",
    "{% ifchanged %}a{% endifchanged x %}",
    "{% if a %}{% elif %}{% endif %}",
    "{% for x in l %}{% else %}{% endfor %}",
    "{{ 1. }}",
    "{{ s|default:1. }}",
    "{{ es|default:1_000 }}",
    "{{ es|default:1e309 }}",
    "{{ es|default:-1 }}",
    "{{ +1.5 }}",
    "{{ 1_0e1_0 }}",
    "{{ 1._5 }}",
    "{{ 1e }}",
    "{{ e1 }}",
    "{{ 007 }}",
    "{{ -0 }}",
    "{{ 1e5|add:1 }}",
    "{{ .5 }}",
    "{{ 5.e }}",
    "{{ s|upper|lower }}",
    "{{ s |upper }}",
    "{{ s| upper }}",
    "{{ s|upper |lower }}",
    "{{ s|upper | lower }}",
    "{{ s|upper |lower | title }}",
    "{{ s|default :es }}",
    "{{ s|default:1__0 }}",
    "{{ s|default:1.2.3 }}",
    "{{ es|default:1.2.3 }}",
    "{{ s|default:2. }}",
    "{{ d. }}",
    "{% with a=1 b=2 %}{{ a }}{{ b }}{% endwith %}",
    "{% with 1 as b %}{{ b }}{% endwith %}",
    "{% with =1 %}{% endwith %}",
    "{% for x in l reversed %}{{ x }}{% endfor %}",
    "{% if a %}y{% endif %}",
    "{% for x in es %}{% empty %}e{% endfor %}",
    "{% firstof 'a'|upper %}",
    '{% widthratio 1|add:1 "4"|add:0 100 %}',
    "{% load p %}{% st 1 a=2 %}",
    '{% load p %}{% st "a"|upper 2|add:1 %}',
    "{% cycle 1 2 as n %}{% cycle n %}",
    '{% cycle "a"|upper "b" as c silent %}{{ c }}',
]


@pytest.fixture(autouse=True)
def library(monkeypatch):
    module = ModuleType(MODULE)
    module.register = Library()

    @module.register.simple_tag
    def st(*args, **kwargs):
        return f"{args}{sorted(kwargs.items())}"

    monkeypatch.setitem(sys.modules, MODULE, module)


def values():
    return {
        "s": "str",
        "es": "",
        "l": [10, 20, 30],
        "n": None,
        "a": 1,
        "reversed": [1, 2],
        "d": {"1_0": "k", "": "empty"},
    }


def outcome(render):
    try:
        return render()
    except Exception as error:
        return type(error)


@pytest.mark.parametrize("source", SOURCES)
def test_matches_django(source):
    options = {"libraries": {"p": MODULE}}
    expected = outcome(
        lambda: DjangoEngine(**options).from_string(source).render(Context(values()))
    )
    assert outcome(lambda: Engine(**options).from_string(source).render(values())) == (
        expected
    )


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("{% cycle 'a'|upper 'b' %}", "A"),
        ("{{ es|default:1_0 }}", "10"),
        ("{{ 1e309 }}", "inf"),
        ("{{ +1 }}", "1"),
        ("{{ s. }}", ""),
        ("{{ d. }}", "empty"),
    ],
)
def test_renders(source, expected):
    assert Engine().from_string(source).render(values()) == expected


@pytest.mark.parametrize(
    "source",
    [
        "{{ s|upper| }}",
        "{% if a %}{% endif x %}",
        "{% for x in l %}{% empty x %}{% endfor %}",
        "{% with a=1 as b %}{{ a }}{% endwith %}",
        "{% for x in reversed %}{{ x }}{% endfor %}",
        "{{ - }}",
        "{{ -s }}",
        "{% load p %}{% st =1 %}",
    ],
)
def test_is_a_syntax_error(source):
    with pytest.raises(TemplateSyntaxError):
        Engine(libraries={"p": MODULE}).from_string(source)
