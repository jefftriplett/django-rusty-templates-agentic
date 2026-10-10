import pytest
from django.utils import translation


@pytest.mark.parametrize(
    "source",
    [
        r"""{{ "a\"b" }}""",
        r"""{{ missing|default:"a\"b" }}""",
        r"""{% firstof "a\"b" %}""",
    ],
)
def test_escaped_quotes(assert_render, source):
    assert_render(source, {}, 'a"b')


def test_plain_text_keeps_backslashes(assert_render):
    assert_render(r"""a\"b""", {}, r"""a\"b""")


@pytest.mark.parametrize(
    "source",
    [
        '{{ _("Password") }}',
        '{% cycle _("Password") "unused" %}',
        '{{ missing|default:_("Password") }}',
    ],
)
def test_translated_literal_runtime(assert_render, source):
    with translation.override("de"):
        assert_render(source, {}, "Passwort")
    with translation.override("en"):
        assert_render(source, {}, "Password")
