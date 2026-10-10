import pytest


@pytest.mark.parametrize("autoescape", ["on", "off"])
@pytest.mark.parametrize("values,expected", [(["<b>", "&"], "<b>,&"), ([], "")])
def test_safeseq(assert_render, autoescape, values, expected):
    assert_render(
        template="{% autoescape "
        + autoescape
        + ' %}{{ values|safeseq|join:"," }}{% endautoescape %}',
        context={"values": values},
        expected=expected,
    )
