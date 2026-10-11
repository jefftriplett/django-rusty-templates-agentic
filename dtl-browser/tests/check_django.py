"""Validate the shared fixtures against Django, independently of Rust/Wasm."""

import json
from pathlib import Path

from django.conf import settings
from django.template import Context, Engine

if not settings.configured:
    settings.configure(USE_I18N=False, USE_TZ=False)

cases = json.loads(Path(__file__).with_name("parity.json").read_text())
cases.append(
    json.loads((Path(__file__).parents[1] / "demo" / "catalog.json").read_text())
)
for case in cases:
    engine = Engine(
        debug=False,
        loaders=[
            (
                "django.template.loaders.locmem.Loader",
                {**case.get("templates", {}), case["name"]: case["source"]},
            )
        ],
    )
    actual = engine.get_template(case["name"]).render(
        Context(case["context"], use_l10n=False, use_tz=False)
    )
    assert actual == case["expected"], (case["name"], actual, case["expected"])
print(f"Django parity: {len(cases)} cases passed")
