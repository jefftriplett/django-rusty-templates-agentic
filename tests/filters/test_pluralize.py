from decimal import Decimal

import pytest
from django.utils.safestring import mark_safe


@pytest.mark.parametrize(
    "value,expected",
    [
        (0, "s"),
        (1, ""),
        (2, "s"),
        (True, ""),
        (False, "s"),
        ("1", ""),
        ("1.0", ""),
        ("0", "s"),
        ("bad", ""),
        (Decimal("1"), ""),
        (Decimal("1.5"), "s"),
        ([], "s"),
        ([1], ""),
        ([1, 2], "s"),
        ({"a": 1}, ""),
        (None, ""),
        (object(), ""),
    ],
)
def test_pluralize(assert_render, value, expected):
    assert_render("{{ value|pluralize }}", {"value": value}, expected)


@pytest.mark.parametrize(
    "value,suffix,expected",
    [
        (1, "es", ""),
        (2, "es", "es"),
        (1, "y,ies", "y"),
        (2, "y,ies", "ies"),
        (2, "a,b,c", ""),
        (2, "", ""),
        (2, ",", ""),
        (2, "<b>", "&lt;b&gt;"),
        (2, mark_safe("<b>"), "&lt;b&gt;"),
    ],
)
def test_pluralize_suffix(assert_render, value, suffix, expected):
    assert_render(
        "{{ value|pluralize:suffix }}", {"value": value, "suffix": suffix}, expected
    )


def test_pluralize_chaining(assert_render):
    assert_render('{{ value|pluralize:"y,ies"|upper }}', {"value": 2}, "IES")
