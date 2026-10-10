from unittest.mock import patch

import django
import pytest
from django.template import Context, TemplateDoesNotExist

from django_rusty_templates import Engine

pytestmark = pytest.mark.skipif(
    django.VERSION < (6, 0), reason="Partials require Django 6"
)


def partial_engine(debug=False):
    return Engine(
        debug=debug,
        loaders=[
            (
                "django.template.loaders.cached.Loader",
                [
                    (
                        "django.template.loaders.locmem.Loader",
                        {
                            "parts.html": "{% partialdef greeting %}Hi {{ name }}{% endpartialdef %}",
                            "include.html": "[{% include 'parts.html#greeting' %}]",
                        },
                    )
                ],
            )
        ],
    )


@pytest.mark.parametrize("debug", [False, True])
def test_fragment_include_escapes_and_reuses_full_template(debug):
    engine = partial_engine(debug)
    loader = engine.template_loaders[0].loaders[0]
    with patch.object(loader, "get_contents", wraps=loader.get_contents) as contents:
        assert engine.get_template("parts.html").render(Context()) == ""
        assert (
            engine.get_template("parts.html#greeting").render(
                Context({"name": "<Alice>"})
            )
            == "Hi &lt;Alice&gt;"
        )
        contents.assert_called_once()
    assert (
        engine.render_to_string("include.html", {"name": "<Alice>"})
        == "[Hi &lt;Alice&gt;]"
    )


@pytest.mark.parametrize("debug", [False, True])
def test_missing_template_cache_avoids_repeated_loader_reads(debug):
    engine = partial_engine(debug)
    loader = engine.template_loaders[0].loaders[0]
    with patch.object(loader, "get_contents", wraps=loader.get_contents) as contents:
        for _ in range(2):
            with pytest.raises(TemplateDoesNotExist):
                engine.get_template("missing.html#fragment")
        contents.assert_called_once()
