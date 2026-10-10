import pytest
from django.utils.safestring import mark_safe


@pytest.mark.parametrize(
    "value",
    [
        "www.example.com",
        "me@example.com",
        "<b>www.example.com</b>",
        "https://example.com/a?x=1&y=2",
        "www.example.com.",
        mark_safe("<b>www.example.com</b>"),
    ],
)
@pytest.mark.parametrize("filter_expression", ["urlize", "urlizetrunc:12"])
def test_url_filters_match_django(template_engine, value, filter_expression):
    from django.template import Context, Engine

    source = "{{ value|" + filter_expression + " }}"
    expected = Engine().from_string(source).render(Context({"value": value}))
    assert template_engine.from_string(source).render({"value": value}) == expected
