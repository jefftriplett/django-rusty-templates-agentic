from datetime import datetime
from zoneinfo import ZoneInfo

import pytest
from django.template import Context, Engine as DjangoEngine
from django.utils import timezone, translation
from django_rusty_templates import Engine


@pytest.mark.parametrize("autoescape", [False, True])
@pytest.mark.parametrize("use_l10n", [None, False, True])
@pytest.mark.parametrize("use_tz", [None, False, True])
def test_numeric_and_date_formatting_flag_combinations(autoescape, use_l10n, use_tz):
    source = "{% for value in values %}{{ value }}|{% endfor %}{{ 1234.5 }}"
    values = [1234, 1234.5, datetime(2026, 10, 12, 3, tzinfo=ZoneInfo("UTC"))]
    with translation.override("de"), timezone.override(ZoneInfo("Asia/Tokyo")):
        output = []
        for cls in (DjangoEngine, Engine):
            context = Context(
                {"values": values},
                autoescape=autoescape,
                use_l10n=use_l10n,
                use_tz=use_tz,
            )
            output.append(cls().from_string(source).render(context))
        assert output[0] == output[1]


class HtmlValue:
    def __str__(self):
        return "<value>"


def test_formatting_adapters_do_not_leak_autoescape_between_scopes():
    source = (
        "{{ value }}|{% autoescape off %}{{ value }}{% endautoescape %}|{{ value }}"
    )
    for cls in (DjangoEngine, Engine):
        template = cls().from_string(source)
        assert template.render(Context({"value": HtmlValue()})) == (
            "&lt;value&gt;|<value>|&lt;value&gt;"
        )
