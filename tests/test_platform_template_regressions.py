"""Regressions exposed by the complete OS and Django development matrix."""

import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("newline", ["\n", "\r\n", "\r"])
def test_filesystem_templates_use_universal_newlines(
    tmp_path, engine_class, debug, newline
):
    source = "Café" + newline + "{{ value }}" + newline
    (tmp_path / "page.html").write_bytes(source.encode("utf-8"))
    template = engine_class(dirs=[tmp_path], debug=debug).get_template("page.html")
    assert template.source == "Café\n{{ value }}\n"
    assert template.render(Context({"value": "<"})) == "Café\n&lt;\n"


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
@pytest.mark.parametrize(
    "source,expected",
    [
        ("{% comment %}{{% endcomment %}visible", "visible"),
        ("{% verbatim %}\n{%\n{% endverbatim %}", "\n{%\n"),
        ("literal {{ missing {% firstof 'visible' %}", "literal {{ missing visible"),
    ],
)
def test_literal_openers_do_not_hide_valid_tags(engine_class, source, expected):
    assert engine_class().from_string(source).render(Context()) == expected


def test_verbatim_literal_delimiters_match_django_version():
    source = "{% verbatim %}{%{% endverbatim %}text{% verbatim %}%}{% endverbatim %}"
    expected = DjangoEngine().from_string(source).render(Context())
    assert Engine().from_string(source).render(Context()) == expected
