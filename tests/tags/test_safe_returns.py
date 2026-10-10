from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine


@pytest.mark.parametrize("engine_cls", [Engine, DjangoEngine])
@pytest.mark.parametrize("autoescape", [True, False])
def test_custom_tag_returns_preserve_safety_and_types(
    engine_cls, autoescape, monkeypatch
):
    import sys

    library = Library()
    library.simple_tag(lambda: mark_safe("<b>safe</b>"), name="safe_output")
    library.simple_tag(lambda: "<b>raw</b>", name="raw_output")
    library.simple_tag(lambda: 4, name="number")
    library.simple_block_tag(lambda content: content, name="echo")
    module = ModuleType("test_safe_return_tags")
    module.register = library
    monkeypatch.setitem(sys.modules, module.__name__, module)
    engine = engine_cls(builtins=[module.__name__], autoescape=autoescape)
    source = "{% safe_output as value %}{{ value }}|{% raw_output %}|{% number as result %}{{ result|add:2 }}|{% echo %}{{ text }}{% endecho %}"
    escaped = "&lt;b&gt;raw&lt;/b&gt;" if autoescape else "<b>raw</b>"
    body = "Jack &amp; Jill" if autoescape else "Jack & Jill"
    assert (
        engine.from_string(source).render(
            Context({"text": "Jack & Jill"}, autoescape=autoescape)
        )
        == f"<b>safe</b>|{escaped}|6|{body}"
    )


def test_escape_and_upper_safety(assert_render):
    assert_render(
        "{{ value|escape }}|{{ value|upper }}",
        {"value": mark_safe("a & b")},
        "a & b|A &amp; B",
    )


def test_force_escape_block_escapes_rendered_content(assert_render):
    assert_render(
        "{% filter force_escape %}{{ value }} & {{ value }}{% endfilter %}",
        {"value": "A & B"},
        "A &amp;amp; B &amp; A &amp;amp; B",
    )
