import pytest


@pytest.mark.parametrize(
    ("template", "expected"),
    [
        (
            '{% cycle "a" "b" as c silent %}{% with x=1 %}{% cycle c %}{% endwith %}{{ c }}',
            "b",
        ),
        (
            '{% cycle "a" "b" as c silent %}{% with x=1 %}{% with y=2 %}{% cycle c %}{% endwith %}{% endwith %}{{ c }}',
            "b",
        ),
        (
            '{% cycle "a" "b" as c silent %}{% with c="local" %}{% cycle c %}{% endwith %}{{ c }}',
            "a",
        ),
        ('{% with x=1 %}{% cycle "a" "b" as c silent %}{% endwith %}{{ c }}', ""),
        (
            '{% with x=1 %}{% firstof "local" as value %}{% endwith %}{{ value }}',
            "outer",
        ),
    ],
)
def test_cycle_and_assignment_scope(assert_render, template, expected):
    assert_render(template, {"value": "outer"}, expected)
