import datetime

import pytest
from django.template import Context, RequestContext, Engine as DjangoEngine
from django.test import RequestFactory
from django.utils import translation, timezone

from django_rusty_templates import Engine


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
def test_context_stack(engine_class):
    context = Context({"value": "base"})
    context.push({"value": "top"})
    assert engine_class().from_string("{{ value }}").render(context) == "top"
    assert context["value"] == "top"


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
@pytest.mark.parametrize("autoescape,expected", [(True, "&lt;b&gt;"), (False, "<b>")])
def test_context_autoescape(engine_class, autoescape, expected):
    assert (
        engine_class()
        .from_string("{{ value }}")
        .render(Context({"value": "<b>"}, autoescape=autoescape))
        == expected
    )


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
@pytest.mark.parametrize("use_l10n,expected", [(True, "1,5"), (False, "1.5")])
def test_context_localization(engine_class, use_l10n, expected):
    with translation.override("de"):
        output = (
            engine_class()
            .from_string("{{ value }}")
            .render(Context({"value": 1.5}, use_l10n=use_l10n))
        )
    assert output == expected


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
@pytest.mark.parametrize("use_tz,hour", [(True, 14), (False, 12)])
def test_context_timezone(engine_class, use_tz, hour):
    value = datetime.datetime(2024, 6, 1, 12, tzinfo=datetime.timezone.utc)

    with timezone.override(datetime.timezone(datetime.timedelta(hours=2))):
        output = (
            engine_class()
            .from_string("{{ value }}")
            .render(Context({"value": value}, use_tz=use_tz, use_l10n=False))
        )
        expected = (
            DjangoEngine()
            .from_string("{{ value }}")
            .render(Context({"value": value}, use_tz=use_tz, use_l10n=False))
        )
    assert output == expected
    assert ("2 p.m." if hour == 14 else "noon") in output


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
def test_request_context_processor_precedence(engine_class):
    calls = []

    def processor(request):
        calls.append(request.path)
        return {"value": "processor"}

    request = RequestFactory().get("/test/")
    context = RequestContext(request, {"value": "initial"}, processors=[processor])
    context.push({"value": "explicit"})
    template = engine_class().from_string("{{ value }}")
    assert template.render(context) == "explicit"
    assert calls == ["/test/"]
    assert context["value"] == "explicit"


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
def test_request_context_initial_values_below_processors(engine_class):
    context = RequestContext(
        RequestFactory().get("/"),
        {"value": "initial"},
        processors=[lambda request: {"value": "processor"}],
    )
    assert engine_class().from_string("{{ value }}").render(context) == "processor"


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
def test_bound_request_context_processors_run_once(engine_class):
    calls = []

    def processor(request):
        calls.append(request.path)
        return {"value": "processor"}

    context = RequestContext(RequestFactory().get("/"), processors=[processor])
    binding_template = DjangoEngine().from_string("")
    with context.bind_template(binding_template):
        assert engine_class().from_string("{{ value }}").render(context) == "processor"
    assert calls == ["/"]


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
def test_include_only_preserves_context_flags(engine_class):
    engine = engine_class(
        loaders=[("django.template.loaders.locmem.Loader", {"child": "{{ value }}"})]
    )
    template = engine.from_string('{% include "child" with value=value only %}')
    assert template.render(Context({"value": "<b>"}, autoescape=False)) == "<b>"
