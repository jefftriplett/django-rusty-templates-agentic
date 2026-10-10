import pytest
from django.template import Context, Engine as DjangoEngine, TemplateSyntaxError

from django_rusty_templates import Engine


@pytest.mark.parametrize(
    "expression,values,expected",
    [
        (
            "key",
            [{"key": "a"}, {"key": "a"}, {"key": "b"}, {"key": "a"}],
            "a:2;b:1;a:1;",
        ),
        (
            'key|join:"-"',
            [{"key": ["a", "b"]}, {"key": ["a", "b"]}, {"key": ["c"]}],
            "a-b:2;c:1;",
        ),
        ("missing", [{"key": "a"}, {"key": "b"}], "None:2;"),
        ("key", [], ""),
        ("key", None, ""),
    ],
)
def test_regroup(expression, values, expected):
    source = (
        "{% regroup values by "
        + expression
        + " as groups %}{% for key, items in groups %}{{ key }}:{{ items|length }};{% endfor %}"
    )
    for engine in (Engine(), DjangoEngine()):
        assert (
            engine.from_string(source).render(Context({"values": values})) == expected
        )


@pytest.mark.parametrize(
    "source",
    [
        "{% regroup values key as groups %}",
        "{% regroup values wrong key as groups %}",
        "{% regroup values by key wrong groups %}",
    ],
)
def test_regroup_syntax(source):
    for engine in (Engine(), DjangoEngine()):
        with pytest.raises(TemplateSyntaxError):
            engine.from_string(source)
