import pytest
from django.template import Context, Engine as DjangoEngine
from django.utils.functional import lazy
from django.utils.safestring import mark_safe
from django.utils import translation

from django_rusty_templates import Engine
from django_rusty_templates.interop import compile_template


class HtmlString(str):
    def __html__(self):
        return "<strong>safe</strong>"


class DisplayString(str):
    def __str__(self):
        return "<em>display</em>"


@pytest.mark.parametrize(
    "value",
    [
        'Café & <b>"quoted"\'s</b>',
        mark_safe("<strong>safe</strong>"),
        HtmlString("<em>raw</em>"),
        DisplayString("original"),
        lazy(lambda: "<em>lazy</em>", str)(),
    ],
)
@pytest.mark.parametrize("autoescape", [True, False])
@pytest.mark.parametrize("interop", [False, True])
def test_string_rendering_matches_django_across_escape_scopes(
    value, autoescape, interop
):
    source = (
        "{{ value }}|{% autoescape off %}{{ value }}{% endautoescape %}|"
        "{% autoescape on %}{{ value }}{% endautoescape %}|{{ value }}"
    )
    with translation.override("de"):
        kwargs = {"autoescape": autoescape, "use_l10n": True, "use_tz": True}
        expected = (
            DjangoEngine()
            .from_string(source)
            .render(Context({"value": value}, **kwargs))
        )
        template = (
            compile_template(source, Engine())
            if interop
            else Engine().from_string(source)
        )
        assert template.render(Context({"value": value}, **kwargs)) == expected
