from datetime import date, timedelta

import pytest
from django.utils.safestring import mark_safe


@pytest.mark.parametrize(
    ("value", "expected"),
    [("a&b", " a&amp;b "), (mark_safe("a&b"), " a&b "), ("é", "  é  ")],
)
def test_center_preserves_raw_width_and_safety(assert_render, value, expected):
    assert_render("{{ value|center:5 }}", {"value": value}, expected)


def test_default_if_none_missing_in_condition(assert_render):
    assert_render(
        '{% if missing|default_if_none:"fallback" %}yes{% endif %}', {}, "yes"
    )


def test_default_if_none_missing_in_output(assert_render):
    assert_render('{{ missing|default_if_none:"fallback" }}', {}, "")


def test_added_date_uses_template_formatting(assert_render):
    assert_render(
        "{{ value|add:offset }}",
        {"value": date(2026, 1, 2), "offset": timedelta(days=1)},
        "Jan. 3, 2026",
    )
