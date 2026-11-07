from django import VERSION
import pytest
from django.http import QueryDict
from django.template import Context, Engine as DjangoEngine, RequestContext
from django.test import RequestFactory

from django_rusty_templates import Engine


@pytest.mark.parametrize(
    "source,values,expected",
    [
        ("{% querystring params %}", {"params": QueryDict("a=1&b=2")}, "?a=1&amp;b=2"),
        ("{% querystring params a=None %}", {"params": QueryDict("a=1&b=2")}, "?b=2"),
        (
            "{% querystring params a=items %}",
            {"params": QueryDict(""), "items": ["one", "two"]},
            "?a=one&amp;a=two",
        ),
        (
            "{% querystring params %}",
            {"params": QueryDict("")},
            "?" if VERSION >= (6, 0) else "",
        ),
        ("{% querystring params a=None %}", {"params": QueryDict("a=1")}, "?"),
        (
            '{% querystring params b="a b" as result %}{{ result }}',
            {"params": QueryDict("a=1")},
            "?a=1&amp;b=a+b",
        ),
    ],
)
def test_querystring(source, values, expected):
    for engine in (Engine(), DjangoEngine()):
        assert engine.from_string(source).render(Context(values)) == expected
    assert values["params"] == QueryDict(values["params"].urlencode())


def test_querystring_request():
    request = RequestFactory().get("/?a=1&page=2")
    for engine in (Engine(), DjangoEngine()):
        assert (
            engine.from_string("{% querystring page=3 %}").render(
                RequestContext(request)
            )
            == "?a=1&amp;page=3"
        )


def test_querystring_requires_request_or_mapping():
    for engine in (Engine(), DjangoEngine()):
        with pytest.raises(
            AttributeError, match="'Context' object has no attribute 'request'"
        ):
            engine.from_string("{% querystring %}").render(Context())


@pytest.mark.parametrize(
    "query",
    ["", "a=1", "a=1&a=2&b=x", "q=caf%C3%A9+au+lait&x=%26%3D%3F%2B", "e=&f=%7E._-"],
)
@pytest.mark.parametrize(
    "arguments",
    [
        "",
        "page=2",
        "a=None",
        "a=None b=None",
        "a=values",
        "c=values",
        "a=empty",
        "a=nones",
        "z=3 a=4",
        "text='a b&c=d'",
        "flag=True number=1.5",
        "safe=safe",
    ],
)
@pytest.mark.parametrize("autoescape", ["on", "off"])
def test_querystring_matches_django(query, arguments, autoescape):
    from django.utils.safestring import mark_safe

    source = (
        f"{{% autoescape {autoescape} %}}{{% querystring {arguments} %}}|"
        f"{{% querystring {arguments} as qs %}}{{{{ qs }}}}{{% endautoescape %}}"
    )
    context = {
        "values": ["x", 2, "é"],
        "empty": [],
        "nones": [None, "kept"],
        "safe": mark_safe("<b>"),
    }
    request = RequestFactory().get("/", QueryDict(query))
    outputs = [
        engine_class().from_string(source).render(RequestContext(request, context))
        for engine_class in (Engine, DjangoEngine)
    ]
    assert outputs[0] == outputs[1]


@pytest.mark.parametrize("value", ["object", "mapping", "lazy"])
def test_querystring_other_values_use_django(value):
    from django.utils.translation import gettext_lazy

    class Thing:
        def __str__(self):
            return "thing"

    values = {
        "object": Thing(),
        "mapping": {"k": "v"},
        "lazy": gettext_lazy("Hello"),
    }
    source = "{% querystring a=value %}"
    request = RequestFactory().get("/?a=1")
    outputs = [
        engine_class()
        .from_string(source)
        .render(RequestContext(request, {"value": values[value]}))
        for engine_class in (Engine, DjangoEngine)
    ]
    assert outputs[0] == outputs[1]
