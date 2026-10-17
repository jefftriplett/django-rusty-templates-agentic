"""Check template syntax for every registered builtin, including Python-only tests."""

import inspect

import pytest
from django.template import Engine as DjangoEngine
from django.template.defaultfilters import register as filters
from django.template.defaulttags import register as tags
from django.template.loader_tags import register as loader_tags

from django_rusty_templates import Engine


@pytest.mark.parametrize("name", sorted(filters.filters))
def test_every_default_filter_compiles_in_rusty(name):
    parameters = list(inspect.signature(filters.filters[name]).parameters.values())
    required_argument = any(
        parameter.default is inspect.Parameter.empty
        and parameter.kind
        in (parameter.POSITIONAL_ONLY, parameter.POSITIONAL_OR_KEYWORD)
        for parameter in parameters[1:]
    )
    expression = name + (":argument" if required_argument else "")
    source = "{{ value|" + expression + " }}"
    DjangoEngine().from_string(source)
    assert Engine().from_string(source)._is_native


TAG_SAMPLES = {
    "autoescape": "{% autoescape off %}body{% endautoescape %}",
    "block": "{% block body %}body{% endblock %}",
    "comment": "{% comment %}body{% endcomment %}",
    "csp_nonce_attr": "{% csp_nonce_attr %}",
    "csrf_token": "{% csrf_token %}",
    "cycle": "{% cycle 'a' 'b' %}",
    "debug": "{% debug %}",
    "extends": "{% extends 'parent.html' %}",
    "filter": "{% filter upper %}body{% endfilter %}",
    "firstof": "{% firstof value 'fallback' %}",
    "for": "{% for value in values %}{{ value }}{% endfor %}",
    "if": "{% if value %}body{% endif %}",
    "ifchanged": "{% ifchanged value %}body{% endifchanged %}",
    "include": "{% include 'child.html' %}",
    "load": "{% load %}",
    "lorem": "{% lorem 1 w %}",
    "now": "{% now 'Y' %}",
    "partial": "{% partialdef example %}body{% endpartialdef %}{% partial example %}",
    "partialdef": "{% partialdef example %}body{% endpartialdef %}",
    "querystring": "{% querystring params %}",
    "regroup": "{% regroup values by category as groups %}",
    "resetcycle": "{% cycle 'a' 'b' %}{% resetcycle %}",
    "spaceless": "{% spaceless %}<p>body</p>{% endspaceless %}",
    "templatetag": "{% templatetag openvariable %}",
    "url": "{% url 'home' %}",
    "verbatim": "{% verbatim %}{{ value }}{% endverbatim %}",
    "widthratio": "{% widthratio value total 100 %}",
    "with": "{% with value='body' %}{{ value }}{% endwith %}",
}


@pytest.mark.parametrize("name", sorted(tags.tags.keys() | loader_tags.tags.keys()))
def test_every_default_tag_compiles_in_rusty(name):
    source = TAG_SAMPLES[name]
    DjangoEngine().from_string(source)
    Engine().from_string(source)
