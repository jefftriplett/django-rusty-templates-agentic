import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine


@pytest.mark.parametrize(
    "source,context,expected",
    [
        (
            "{% for x in values %}{% ifchanged x %}{{ x }}{% else %}-{% endifchanged %}{% endfor %}",
            {"values": [1, 1, 2, 2, 1]},
            "1-2-1",
        ),
        (
            "{% for x in values %}{% ifchanged %}{{ x }}{% else %}-{% endifchanged %}{% endfor %}",
            {"values": [1, 1, 2]},
            "1-2",
        ),
        (
            "{% for outer in values %}{% for x in values %}{% ifchanged x %}{{ x }}{% endifchanged %}{% endfor %}|{% endfor %}",
            {"values": [1, 1, 2]},
            "12|12|12|",
        ),
        (
            "{% for x in values %}{% with y=x %}{% ifchanged y %}{{ y }}{% endifchanged %}{% endwith %}{% endfor %}",
            {"values": [1, 1, 2]},
            "12",
        ),
        (
            "{% for x in values %}{% ifchanged missing %}Y{% else %}N{% endifchanged %}{% endfor %}",
            {"values": [1, 1]},
            "YN",
        ),
        ("{% ifchanged %}once{% endifchanged %}", {}, "once"),
        (
            "{% for x in values %}{% ifchanged x.a x.b %}Y{% else %}N{% endifchanged %}{% endfor %}",
            {"values": [{"a": 1, "b": 2}, {"a": 1, "b": 2}, {"a": 1, "b": 3}]},
            "YNY",
        ),
    ],
)
def test_ifchanged(source, context, expected):
    for engine in (Engine(), DjangoEngine()):
        template = engine.from_string(source)
        assert template.render(Context(context)) == expected
        assert template.render(Context(context)) == expected


def test_separate_include_nodes_keep_separate_state():
    for engine_class in (Engine, DjangoEngine):
        engine = engine_class(
            loaders=[
                (
                    "django.template.loaders.locmem.Loader",
                    {
                        "parent": '{% for x in values %}{% include "child" %}{% include "child" %}{% endfor %}',
                        "child": "{% ifchanged %}{{ x }}{% endifchanged %}",
                    },
                )
            ]
        )
        template = engine.get_template("parent")
        assert template.render(Context({"values": [1, 1, 2, 2, 3, 3]})) == "112233"
