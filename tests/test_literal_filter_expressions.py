import sys
from types import ModuleType

import pytest
from django.template import (
    Context,
    Engine as DjangoEngine,
    Library,
    TemplateSyntaxError,
)

from django_rusty_templates import Engine


@pytest.mark.parametrize(
    "source,expected",
    [
        ('{% if "foo"|length == 3 %}yes{% endif %}', "yes"),
        ("{% if 'a b'|cut:' '|upper == 'AB' and enabled %}yes{% endif %}", "yes"),
        ("{% if 0|default:enabled %}yes{% endif %}", "yes"),
        ('{% if "élève"|length == 5 %}yes{% endif %}', "yes"),
        ('{% if "x"|default:_("unused words") %}yes{% endif %}', "yes"),
        (
            '{% if "ab"|cut:"b" == "a" %}yes{% elif "ab"|length == 0 %}no{% endif %}',
            "yes",
        ),
        ('{% for value in "ab"|make_list %}{{ value }}{% endfor %}', "ab"),
        (
            '{% for value in "ab cd"|cut:" "|make_list reversed %}{{ value }}{% endfor %}',
            "dcba",
        ),
        ('{% for value in ""|make_list %}no{% empty %}empty{% endfor %}', "empty"),
        ("{% for value in 12|make_list %}{{ value }}{% endfor %}", "12"),
        ('{% for value in "é世"|make_list %}{{ value }}{% endfor %}', "é世"),
        (r"""{% if 'a" b'|cut:'" '|length == 2 %}yes{% endif %}""", "yes"),
    ],
)
@pytest.mark.parametrize("inherited", [False, True])
def test_literal_filter_expressions(source, expected, inherited):
    if inherited:
        source = '{% extends "base.html" %}{% block body %}' + source + "{% endblock %}"
    loaders = [
        (
            "django.template.loaders.locmem.Loader",
            {"base.html": "{% block body %}{% endblock %}"},
        )
    ]
    for cls in (DjangoEngine, Engine):
        engine = cls(loaders=loaders)
        assert engine.from_string(source).render(Context({"enabled": True})) == expected


@pytest.mark.parametrize(
    "source",
    [
        '{% if "ab"| %}no{% endif %}',
        '{% if "ab"|unknown_filter %}no{% endif %}',
        '{% if "ab"|cut: %}no{% endif %}',
        '{% for value in "ab"| %}no{% endfor %}',
        '{% for value in "ab"|unknown_filter %}no{% endfor %}',
        '{% for value in "ab"|cut: %}no{% endfor %}',
        "{{ value|cut: }}",
    ],
)
def test_invalid_literal_filter_expressions_still_raise(source):
    for cls in (DjangoEngine, Engine):
        with pytest.raises(TemplateSyntaxError):
            cls().from_string(source)


@pytest.mark.parametrize("inherited", [False, True])
def test_literal_template_names_with_loaded_filter(monkeypatch, inherited):
    library = ModuleType("literal_audit_filters")
    library.register = Library()
    library.register.filter("template_name", lambda value: value.replace("oo", ""))
    monkeypatch.setitem(sys.modules, library.__name__, library)
    if inherited:
        source = (
            '{% extends "foo.txt"|template_name %}{% block body %}child{% endblock %}'
        )
        expected = "parent:child"
    else:
        source = '{% load audit %}{% include "foo.txt"|template_name %}'
        expected = "parent:base"
    options = {
        "libraries": {"audit": library.__name__},
        "loaders": [
            (
                "django.template.loaders.locmem.Loader",
                {
                    "f.txt": "parent:{% block body %}base{% endblock %}",
                },
            )
        ],
    }
    if inherited:
        options["builtins"] = [library.__name__]
    for cls in (DjangoEngine, Engine):
        assert cls(**options).from_string(source).render(Context()) == expected
