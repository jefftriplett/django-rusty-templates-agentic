import datetime

import pytest
from django.template import Context, Engine as DjangoEngine, RequestContext
from django.test import RequestFactory
from django.utils import timezone, translation

from django_rusty_templates import Engine
from django_rusty_templates.interop import compile_template


def render_fragment_template(source, context):
    return compile_template(source, Engine()).render(context)


def test_fragment_reads_live_loop_and_filter_argument_scopes():
    source = (
        "{{ value|default:fallback }}|"
        "{% for value in values %}{{ value|default:fallback }};{% endfor %}|"
        '{% with fallback="inner" %}{{ missing|default:fallback }}{% endwith %}|'
        '{{ missing|default:fallback }}|{{ True }}|{{ "literal" }}'
    )
    values = {"value": "", "fallback": "outer", "values": ["", "<b>"]}
    expected = DjangoEngine().from_string(source).render(Context(values))
    assert render_fragment_template(source, Context(values)) == expected


@pytest.mark.parametrize("autoescape", [True, False])
@pytest.mark.parametrize("use_l10n", [True, False])
def test_fragment_preserves_context_formatting(autoescape, use_l10n):
    source = "{{ value }}|{{ number }}|{{ date }}"
    values = {
        "value": "<b>",
        "number": 1.5,
        "date": datetime.datetime(2024, 6, 1, 12, tzinfo=datetime.timezone.utc),
    }
    with translation.override("de"), timezone.override("Europe/Berlin"):
        kwargs = {"autoescape": autoescape, "use_l10n": use_l10n, "use_tz": True}
        expected = DjangoEngine().from_string(source).render(Context(values, **kwargs))
        assert render_fragment_template(source, Context(values, **kwargs)) == expected


def test_fragment_does_not_flatten_or_copy_unreferenced_context():
    class Unrelated:
        def __str__(self):
            raise AssertionError("Unreferenced context value was rendered")

    class LiveContext(Context):
        def flatten(self):
            raise AssertionError("Fragment should read the live scope")

    context = LiveContext({"value": "base", "unrelated": Unrelated()})
    template = compile_template("{{ value }}", Engine())
    assert template.render(context) == "base"
    with context.push(value="top"):
        assert template.render(context) == "top"
    assert template.render(context) == "base"


def test_fragment_uses_bound_request_processors_once():
    calls = []

    def processor(request):
        calls.append(request.path)
        return {"value": "processor"}

    context = RequestContext(RequestFactory().get("/fragment/"), processors=[processor])
    assert render_fragment_template("{{ value }} {{ value }}", context) == (
        "processor processor"
    )
    assert calls == ["/fragment/"]
