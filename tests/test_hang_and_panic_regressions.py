"""Templates that used to hang or panic where Django raises or renders (#411)."""

import pytest
from django.template import TemplateSyntaxError


class RaisingIterator:
    def __init__(self):
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls > 3:
            pytest.fail("the loop kept iterating after the iterator raised")
        raise ValueError("broken iterator")


class RaisingAfterTwo(RaisingIterator):
    def __next__(self):
        if self.calls < 2:
            self.calls += 1
            return self.calls
        return super().__next__()


class RaisingGetItem:
    def __getitem__(self, index):
        raise KeyError("broken index")


@pytest.mark.parametrize(
    ("value", "error"),
    [
        (RaisingIterator, ValueError),
        (RaisingAfterTwo, ValueError),
        (RaisingGetItem, KeyError),
    ],
)
@pytest.mark.parametrize("reverse", ["", " reversed"])
def test_for_stops_at_an_iterators_first_error(template_engine, value, error, reverse):
    template = template_engine.from_string(
        f"{{% for x in v{reverse} %}}{{{{ x }}}}{{% empty %}}none{{% endfor %}}"
    )
    with pytest.raises(error) as info:
        template.render({"v": value()})
    assert "broken" in str(info.value)


@pytest.mark.parametrize(
    ("format", "expected"),
    [
        ("“Y”", "2026"),
        ("éYé", "2026"),
        ("😀Y😀", "2026"),
        ("é", "May 4, 2026"),
        ("x😀", "May 4, 2026"),
        ("éé", "May 4, 2026"),
    ],
)
def test_now_trims_one_character_from_each_end(
    assert_render, time_machine, format, expected
):
    # An empty format is Django's default date format.
    time_machine.move_to("2026-05-04 12:00:00")
    assert_render(template=f"{{% now {format} %}}", context={}, expected=expected)


@pytest.mark.parametrize(
    "template",
    [
        "{%}",
        "{#}",
        "{{}",
        "a{%}b",
        "{%}{{ x }}",
        "{#}{{ x }}{#}",
        "{##}",
        "{#}#}",
    ],
)
def test_start_and_end_markers_dont_overlap(render_output, template):
    engines_output = render_output(template=template, context={"x": "X"})
    assert (
        engines_output
        == {
            "{%}": "{%}",
            "{#}": "{#}",
            "{{}": "{{}",
            "a{%}b": "a{%}b",
            "{%}{{ x }}": "{%}X",
            "{#}{{ x }}{#}": "",
            "{##}": "",
            "{#}#}": "",
        }[template]
    )


@pytest.mark.parametrize(
    "template",
    ["{%}%}", "{% if 1 %}{%}{% endif %}", "{%%}", "{{}}", "{{}}}"],
)
def test_overlapping_markers_inside_tags_are_syntax_errors(template_engine, template):
    with pytest.raises(TemplateSyntaxError):
        template_engine.from_string(template)
