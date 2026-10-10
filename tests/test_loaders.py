from django import VERSION
import pytest
from django.template import TemplateSyntaxError
from inline_snapshot import snapshot

from django_rusty_templates import RustyTemplates


def test_extends_locmem_loader(engine_class):
    template = "{{ }}"
    loaders = [
        (
            "django.template.loaders.locmem.Loader",
            {"template": template},
        ),
    ]
    config = {
        "OPTIONS": {"loaders": loaders},
        "NAME": "locmem",
        "DIRS": (),
        "APP_DIRS": False,
    }
    engine = engine_class(config)

    with pytest.raises(TemplateSyntaxError) as exc_info:
        engine.get_template("template")

    expected = "Empty variable tag on line 1"
    if VERSION >= (6, 0) and engine_class != RustyTemplates:
        expected = "Template: template, " + expected
    if engine_class == RustyTemplates:
        assert str(exc_info.value) == expected
        assert exc_info.value.rusty_diagnostic == snapshot("""\
  × Empty variable tag
   ╭─[template:1:1]
 1 │ {{ }}
   · ──┬──
   ·   ╰── here
   ╰────
""")
    else:
        assert str(exc_info.value) == expected
