"""Measure output assembly; loaded and from_string paths stay separate.

Use a release extension. DEBUG and loader caching vary independently; results
are synthetic warm renders, not HTTP or allocation measurements.
--number is a minimum batch size; short operations calibrate to at least 10ms.
Each paired round normalizes per-operation timings across the engines.
"""

import argparse
import hashlib
import importlib.metadata
import json
import platform
import random
import sys
import tempfile
from pathlib import Path
from types import ModuleType

from benchmark_pairs import MEASUREMENT, MINIMUM_DURATION, measure_pair

import django
from django.conf import settings
from django.template import Context, Engine as DjangoEngine, Library
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine


def fixtures():
    cases = {}
    for length in (8, 256, 8192):
        for kind in ("exact", "escaped", "safe", "callback"):
            value = ("<é&>" if kind != "exact" else "text") * (length // 4)
            if kind == "safe":
                value = mark_safe(value)
            source = (
                "{% load output %}" + "{% echo value %}" * 8
                if kind == "callback"
                else "{{ value }}" * 8
            )
            cases[f"{kind}_{length}"] = (source, {"value": value})
    values = {"value": "<é&>", "items": list(range(8)), "inner": list(range(4))}
    cases.update(
        shallow=("{% for i in items %}{{ value }}{{ i }}{% endfor %}", values),
        deep=(
            "{% for i in inner %}{% for j in inner %}{% for k in inner %}"
            "{{ value }}{{ i }}{{ j }}{{ k }}{% endfor %}{% endfor %}{% endfor %}",
            values,
        ),
        include=("{% for i in items %}{% include 'part.html' %}{% endfor %}", values),
        inheritance=(
            "{% extends 'base.html' %}{% block body %}{{ block.super }}"
            "{% for i in items %}{{ value }}{% endfor %}{% endblock %}",
            values,
        ),
        capture=("{% filter upper %}{{ value }}{{ value }}{% endfilter %}", values),
        asvar=("{% load output %}{% echo value as held %}{{ held }}{{ held }}", values),
    )
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=20)
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--json-output", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--max-regression", type=float, default=0.15)
    args = parser.parse_args()
    if min(args.number, args.repeat) <= 0:
        parser.error("number and repeat must be positive")
    if not settings.configured:
        settings.configure(USE_I18N=False, SECRET_KEY="output-benchmark")
    module = ModuleType("rusty_output_benchmark_tags")
    module.register = Library()
    sys.modules[module.__name__] = module

    @module.register.simple_tag
    def echo(value):
        return value

    cases = fixtures()
    sources = {name + ".html": source for name, (source, _) in cases.items()}
    sources.update(
        {
            "base.html": "<{% block body %}base:{{ value }}{% endblock %}>",
            "part.html": "part:{{ value }}{{ i }}",
        }
    )
    results = {}
    paired_ratios = {}
    random_order = random.Random(0)
    with tempfile.TemporaryDirectory(prefix="rusty-output-") as directory:
        for name, source in sources.items():
            Path(directory, name).write_text(source)
        loader = ("django.template.loaders.filesystem.Loader", [directory])
        for debug in (False, True):
            for cached in (False, True):
                options = dict(
                    debug=debug,
                    libraries={"output": module.__name__},
                    loaders=[("django.template.loaders.cached.Loader", [loader])]
                    if cached
                    else [loader],
                )
                engines = {
                    "django": DjangoEngine(**options),
                    "rusty": Engine(**options),
                }
                for case, (source, values) in cases.items():
                    expected = (
                        engines["django"]
                        .get_template(case + ".html")
                        .render(Context(values))
                    )
                    for path in ("loaded", "from_string"):
                        templates = {
                            arm: engine.get_template(case + ".html")
                            if path == "loaded"
                            else engine.from_string(source)
                            for arm, engine in engines.items()
                        }
                        for arm, template in templates.items():
                            assert template.render(Context(values)) == expected, (
                                case,
                                path,
                                arm,
                            )
                        medians, paired = measure_pair(
                            {
                                arm: (
                                    lambda template=template: template.render(
                                        Context(values)
                                    ),
                                    args.number,
                                    1,
                                )
                                for arm, template in templates.items()
                            },
                            args.repeat,
                            random_order,
                        )
                        key = f"debug={debug}/cached={cached}/{case}/{path}"
                        for arm, median in medians.items():
                            results[f"{key}/{arm}"] = median
                        paired_ratios[key] = paired
    report = {
        "python": platform.python_version(),
        "platform": platform.platform(),
        "django": django.get_version(),
        "rusty_version": importlib.metadata.version("django-rusty-templates-agentic"),
        "fixture_sha256": hashlib.sha256(
            json.dumps(
                {
                    "sources": sources,
                    "values": {name: values for name, (_, values) in cases.items()},
                    "types": {
                        name: {
                            key: type(value).__name__ for key, value in values.items()
                        }
                        for name, (_, values) in cases.items()
                    },
                },
                sort_keys=True,
            ).encode()
        ).hexdigest(),
        "number": args.number,
        "repeat": args.repeat,
        "microseconds": results,
        "measurement": MEASUREMENT,
        "minimum_duration": MINIMUM_DURATION,
        "paired_ratios": paired_ratios,
    }
    if args.json_output:
        args.json_output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if args.baseline:
        base = json.loads(args.baseline.read_text())
        for field in (
            "python",
            "django",
            "fixture_sha256",
            "number",
            "repeat",
            "measurement",
            "minimum_duration",
        ):
            if base.get(field) != report[field]:
                raise SystemExit(f"Incompatible baseline: {field}")
        if base["microseconds"].keys() != results.keys():
            raise SystemExit("Incompatible baseline: cases")
        failures = []
        if base["paired_ratios"].keys() != paired_ratios.keys():
            raise SystemExit("Incompatible baseline: paired cases")
        for key, value in paired_ratios.items():
            ratio = value / base["paired_ratios"][key]
            if ratio > 1 + args.max_regression:
                failures.append(f"{key}: {ratio:.2f}x relative to Django")
        if failures:
            raise SystemExit("Performance regression:\n" + "\n".join(failures))


if __name__ == "__main__":
    main()
