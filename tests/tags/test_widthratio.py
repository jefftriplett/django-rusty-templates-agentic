import pytest
from django.template import TemplateSyntaxError


@pytest.mark.parametrize(
    "value,maximum,width,expected",
    [
        (50, 100, 100, "50"),
        (50, 80, 100, "62"),
        (51, 80, 100, "64"),
        (0, 0, 100, "0"),
        (0, 100, 100, "0"),
        (50, 100, 0, "0"),
        ("50", "100", "100", "50"),
        (None, 100, 100, ""),
        (50, None, 100, ""),
        ("bad", 100, 100, ""),
        (float("inf"), 100, 100, ""),
        (-50, 100, 100, "-50"),
    ],
)
def test_widthratio(assert_render, value, maximum, width, expected):
    assert_render(
        "{% widthratio value maximum width %}",
        {"value": value, "maximum": maximum, "width": width},
        expected,
    )


def test_widthratio_assignment(assert_render):
    assert_render(
        "{% widthratio value 80 100 as ratio %}-{{ ratio }}-", {"value": 50}, "-62-"
    )


@pytest.mark.parametrize(
    "source",
    [
        "{% widthratio %}",
        "{% widthratio a b %}",
        "{% widthratio a b c d %}",
        "{% widthratio a b c as %}",
    ],
)
def test_widthratio_invalid_syntax(template_engine, source):
    with pytest.raises(TemplateSyntaxError):
        template_engine.from_string(source)


@pytest.mark.parametrize("width", [None, "bad"])
def test_widthratio_invalid_width(template_engine, width):
    template = template_engine.from_string("{% widthratio 1 2 width %}")
    with pytest.raises(
        TemplateSyntaxError, match="widthratio final argument must be a number"
    ):
        template.render({"width": width})
