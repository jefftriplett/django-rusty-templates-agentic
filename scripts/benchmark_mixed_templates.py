"""Measure disk-loaded inherited components across DEBUG and loader settings.

Compile uses the engine's loaded-template compiler, not from_string. Loading,
first rendering, warm rendering and uncached load+render are separate phases.
Use a release-built extension; results are synthetic, not HTTP timings.
"""

import argparse
import json
import platform
import statistics
import tempfile
import timeit
from pathlib import Path
from types import SimpleNamespace

import django
from django.conf import settings
from django.template import Context, Engine as DjangoEngine, Origin, Template

from benchmark_loop_values import component_library
from django_rusty_templates import Engine


def fixture():
    row = "{% badge i %}/{{ i }}/{% custom i %}/{{ seen }}/{% card i %}/{% include 'part.html' with value=i %}/{% url 'benchmark_home' %}|"
    return {
        "base.html": "<main>{% block content %}base{% endblock %}</main>{{ seen }}",
        "page.html": '{% extends "base.html" %}{% block content %}{% load components %}{% for i in values %}'
        + row * 3
        + "{% endfor %}{% endblock %}",
        "card.html": "card:{{ value }}",
        "part.html": "part:{{ value }}",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=100)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--json-output", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--max-regression", type=float, default=0.15)
    args = parser.parse_args()
    if min(args.number, args.repeat) <= 0:
        parser.error("number and repeat must be positive")
    if not settings.configured:
        settings.configure(
            USE_I18N=False,
            SECRET_KEY="benchmark",
            ROOT_URLCONF="rusty_component_benchmark_urls",
        )
    libraries = component_library()
    sources = fixture()
    values = {"values": list(range(8))}
    results = {}
    with tempfile.TemporaryDirectory(prefix="rusty-mixed-") as directory:
        for name, source in sources.items():
            Path(directory, name).write_text(source)
        file_loader = ("django.template.loaders.filesystem.Loader", [directory])
        for debug in [False, True]:
            for cached in [False, True]:
                loaders = (
                    [("django.template.loaders.cached.Loader", [file_loader])]
                    if cached
                    else [file_loader]
                )
                reference = DjangoEngine(
                    loaders=loaders, libraries=libraries, debug=debug
                )
                expected = reference.get_template("page.html").render(Context(values))
                for name, kind in [("django", DjangoEngine), ("rusty", Engine)]:
                    engine = kind(loaders=loaders, libraries=libraries, debug=debug)
                    loaded = engine.get_template("page.html")
                    assert loaded.render(Context(values)) == expected
                    origin = loaded.origin
                    if name == "rusty":
                        compile_origin = Origin(
                            str(Path(directory, "page.html")),
                            template_name="page.html",
                            loader=SimpleNamespace(_native=origin.loader),
                        )

                        def compile_template():
                            return engine._compile_loaded_template(
                                sources["page.html"], compile_origin, "page.html"
                            )
                    else:

                        def compile_template():
                            return Template(
                                sources["page.html"],
                                origin=origin,
                                name="page.html",
                                engine=engine,
                            )

                    assert compile_template().render(Context(values)) == expected

                    def compile_and_render():
                        return compile_template().render(Context(values))

                    def load_and_render():
                        return engine.get_template("page.html").render(Context(values))

                    phases = {
                        "read_source": lambda: Path(directory, "page.html").read_text(),
                        "compile": compile_template,
                        "load": lambda: engine.get_template("page.html"),
                        "warm_render": lambda: loaded.render(Context(values)),
                        "compile_and_render": compile_and_render,
                        "load_and_render": load_and_render,
                    }
                    for phase, operation in phases.items():
                        samples = timeit.repeat(
                            operation, number=args.number, repeat=args.repeat
                        )
                        results[f"debug={debug}/cached={cached}/{name}/{phase}"] = (
                            statistics.median(samples) / args.number * 1e6
                        )
    report = {
        "python": platform.python_version(),
        "django": django.get_version(),
        "number": args.number,
        "repeat": args.repeat,
        "microseconds": results,
    }
    if args.json_output:
        args.json_output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        for field in ["python", "django", "number", "repeat"]:
            if baseline[field] != report[field]:
                raise SystemExit(f"Incompatible baseline: {field}")
        failures = []
        for key, value in results.items():
            if "/rusty/" not in key or key.endswith("/read_source"):
                continue
            control = key.replace("/rusty/", "/django/")
            ratio = (value / baseline["microseconds"][key]) / (
                results[control] / baseline["microseconds"][control]
            )
            if ratio > 1 + args.max_regression:
                failures.append(f"{key}: {ratio:.2f}x relative to Django")
        if failures:
            raise SystemExit("Performance regression:\n" + "\n".join(failures))


if __name__ == "__main__":
    main()
