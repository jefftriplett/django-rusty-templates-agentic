import pytest
from django.template import TemplateSyntaxError
from django.test import override_settings


@pytest.mark.parametrize(
    "source,context,expected",
    [
        (
            "{% verbatim %}{{ bare }}{% endif %}{% endverbatim %}",
            {},
            "{{ bare }}{% endif %}",
        ),
        (
            "{% verbatim special %}Don't {% endverbatim %} yet{% endverbatim special %}",
            {},
            "Don't {% endverbatim %} yet",
        ),
        (
            "{% with x=value %}{{ x }}{% endwith %}{{ x }}",
            {"value": "inner", "x": "outer"},
            "innerouter",
        ),
        ("{% with value as x %}{{ x }}{% endwith %}", {"value": "inner"}, "inner"),
        (
            "{% with x=value y=x %}{{ x }}/{{ y }}{% endwith %}",
            {"value": "inner", "x": "outer"},
            "inner/outer",
        ),
        ("{% filter upper|lower %}Hello{% endfilter %}", {}, "hello"),
        (
            "{% filter cut:remove %}hellospam{% endfilter %}",
            {"remove": "spam"},
            "hello",
        ),
        ("{% filter upper %}{{ value }}{% endfilter %}", {"value": "<b>"}, "&LT;B&GT;"),
        (
            "{% spaceless %} <p> hello </p> \n <b>{{ value }}</b> {% endspaceless %}",
            {"value": "<x>"},
            "<p> hello </p><b>&lt;x&gt;</b>",
        ),
        (
            "{% for i in items %}{% cycle 'a' 'b' %}{% resetcycle %}{% endfor %}",
            {"items": range(3)},
            "aaa",
        ),
        (
            "{% cycle 'a' 'b' as letters %}{% resetcycle letters %}{% cycle letters %}",
            {},
            "aa",
        ),
        (
            "{% with 1 as x %}{% cycle 'a' 'b' as letters %}{% endwith %}{% cycle letters %}",
            {},
            "ab",
        ),
    ],
)
def test_small_builtin_rendering(assert_render, source, context, expected):
    assert_render(source, context, expected)


@pytest.mark.parametrize(
    "source",
    [
        "{% resetcycle %}",
        "{% resetcycle unknown %}",
        "{% resetcycle a b %}",
        "{% with %}{% endwith %}",
        "{% with a xx b %}{% endwith %}",
        "{% filter safe %}x{% endfilter %}",
        "{% filter upper|escape %}x{% endfilter %}",
        "{% verbatim %}unclosed",
    ],
)
def test_small_builtin_invalid_syntax(template_engine, source):
    with pytest.raises(TemplateSyntaxError):
        template_engine.from_string(source)


def test_debug_disabled(assert_render):
    with override_settings(DEBUG=False):
        assert_render("{% debug %}", {}, "")


def test_debug_escaping(render_output):
    with override_settings(DEBUG=True):
        output = render_output("{% debug %}", {"value": "<script>"})
    assert output.startswith("{&#x27;value&#x27;: &#x27;&lt;script&gt;&#x27;}")
    assert "&#x27;django&#x27;: &lt;module" in output
