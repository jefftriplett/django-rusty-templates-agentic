"""One template source, verified against Django and both server runtimes."""

import json
from pathlib import Path

import pytest
from django.template.backends.django import DjangoTemplates

from django_rusty_templates import PortableEngine, RustyTemplates

PROTOTYPE = Path(__file__).resolve().parents[1] / "dtl-browser"
CASES = json.loads((PROTOTYPE / "tests" / "parity.json").read_text(encoding="utf-8"))
CASES.append(
    json.loads((PROTOTYPE / "demo" / "catalog.json").read_text(encoding="utf-8"))
)


@pytest.mark.parametrize("case", CASES, ids=lambda case: case["name"])
@pytest.mark.parametrize("runtime", [DjangoTemplates, RustyTemplates, PortableEngine])
def test_shared_template_source(case, runtime):
    templates = {**case.get("templates", {}), case["name"]: case["source"]}
    if runtime is PortableEngine:
        engine = PortableEngine()
        for name, source in templates.items():
            engine.add_template(name, source)
        actual = engine.render_json(case["name"], json.dumps(case["context"]))
    else:
        backend = runtime(
            {
                "NAME": "shared",
                "DIRS": [],
                "APP_DIRS": False,
                "OPTIONS": {
                    "debug": False,
                    "loaders": [("django.template.loaders.locmem.Loader", templates)],
                },
            }
        )
        actual = backend.get_template(case["name"]).render(case["context"])
    assert actual == case["expected"]


def test_portable_errors_and_failed_replacement():
    engine = PortableEngine()
    engine.add_template("page", "Hello {{ name }}")
    with pytest.raises(ValueError, match="Unsupported tag"):
        engine.add_template("page", "{% load i18n %}")
    assert engine.render_json("page", '{"name":"<&"}') == "Hello &lt;&amp;"
    with pytest.raises(ValueError, match="JSON object"):
        engine.render_json("page", "[]")
    engine.add_template("cycle", "{% extends 'cycle' %}")
    with pytest.raises(ValueError, match="Inheritance cycle"):
        engine.render_json("cycle", "{}")
