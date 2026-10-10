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
