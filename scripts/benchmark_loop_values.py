"""Compare loop rendering across engines and template loading paths.

Run after `uv run maturin develop --release`. Optional --baseline compares
Rusty cases to a previous JSON report and fails above --max-regression.
Results are synthetic microseconds per iteration, not HTTP request timings.
"""

import argparse
import json
import platform
import statistics
import sys
import timeit
from pathlib import Path
from types import ModuleType

import django
from django.conf import settings
from django.template import Context, Engine as DjangoEngine, Library, Node
from django.urls import path as url_path

from django_rusty_templates import Engine


def component_library():
    urls = ModuleType("rusty_component_benchmark_urls")
    urls.urlpatterns = [url_path("", lambda request: None, name="benchmark_home")]
    sys.modules[urls.__name__] = urls
    module = ModuleType("rusty_component_benchmark")
    module.register = Library()
    sys.modules[module.__name__] = module

    @module.register.simple_tag
    def badge(value):
        return f"badge:{value}"

    @module.register.inclusion_tag("card.html")
    def card(value):
        return {"value": value}

    class CustomNode(Node):
        def __init__(self, expression):
            self.expression = expression

        def render(self, context):
            context["seen"] = self.expression.resolve(context)
            return f"custom:{context['seen']}"

    @module.register.tag("custom")
    def compile_custom(parser, token):
        return CustomNode(parser.compile_filter(token.split_contents()[1]))

    return {"components": module.__name__}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=200)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--iterations", type=int, default=100)
    parser.add_argument("--json-output", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--max-regression", type=float, default=0.20)
    args = parser.parse_args()
    if min(args.number, args.repeat, args.iterations) <= 0:
        parser.error("number, repeat and iterations must be positive")
    if not settings.configured:
        settings.configure(
            USE_I18N=False,
            SECRET_KEY="benchmark",
            ROOT_URLCONF="rusty_component_benchmark_urls",
        )
    results = {}
    values = {"values": list(range(args.iterations))}
    libraries = component_library()
    for debug in [False, True]:
        for shape, body in [
            ("text", "x"),
            ("one", "{{ i }}"),
            ("two", "{{ i }}{{ i }}"),
            ("captured", "{{ lookup }}"),
            ("captured_large", "{{ lookup }}"),
            (
                "components",
                "{% badge i %}/{{ i }}/{% card i %}/{% custom i %}/{{ seen }}/{% include 'part.html' with value=i %}/{% url 'benchmark_home' %}|",
            ),
        ]:
            source = (
                ("{% load components %}" if shape == "components" else "")
                + "{% for i in values %}"
                + body
                + "{% endfor %}"
            )
            expected = "".join(
                f"badge:{i}/{i}/card:{i}/custom:{i}/{i}/part:{i}//|"
                if shape == "components"
                else "x"
                if shape == "text"
                else str(i)
                * (1 if shape in ("one", "captured", "captured_large") else 2)
                for i in values["values"]
            )
            loader = (
                "django.template.loaders.locmem.Loader",
                {
                    "loop.html": source,
                    "card.html": "card:{{ value }}",
                    "part.html": "part:{{ value }}",
                },
            )
            for name, kind in [("rusty", Engine), ("django", DjangoEngine)]:
                for cached in [True, False]:
                    loaders = (
                        [("django.template.loaders.cached.Loader", [loader])]
                        if cached
                        else [loader]
                    )
                    engine = kind(loaders=loaders, debug=debug, libraries=libraries)
                    for path in ["loaded", "string", "load_and_render"]:
                        template = (
                            engine.from_string(source)
                            if path == "string"
                            else engine.get_template("loop.html")
                        )
                        captured = None
                        if shape in ("captured", "captured_large"):
                            padding = (
                                {f"unused_{i}": i for i in range(100)}
                                if shape == "captured_large"
                                else {}
                            )
                            captured = Context({**values, **padding})

                            def lookup():
                                return captured["i"]

                            captured["lookup"] = lookup

                        def render():
                            current = (
                                engine.get_template("loop.html")
                                if path == "load_and_render"
                                else template
                            )
                            return current.render(
                                captured if captured is not None else Context(values)
                            )

                        for _ in range(3):
                            assert render() == expected
                        samples = timeit.repeat(
                            render, number=args.number, repeat=args.repeat
                        )
                        key = f"debug={debug}/cached={cached}/{shape}/{name}/{path}"
                        results[key] = (
                            statistics.median(samples)
                            / args.number
                            / args.iterations
                            * 1e6
                        )
    fixed_results = {}
    for debug in [False, True]:
        for shape, source in [
            ("flat", "content"),
            (
                "inheritance",
                '{% extends "base.html" %}{% block content %}content{% endblock %}',
            ),
        ]:
            loader = (
                "django.template.loaders.locmem.Loader",
                {
                    "page.html": source,
                    "base.html": "{% block content %}base{% endblock %}",
                },
            )
            for name, kind in [("rusty", Engine), ("django", DjangoEngine)]:
                for cached in [True, False]:
                    loaders = (
                        [("django.template.loaders.cached.Loader", [loader])]
                        if cached
                        else [loader]
                    )
                    engine = kind(loaders=loaders, debug=debug)
                    for render_path in ["loaded", "load_and_render"]:
                        template = engine.get_template("page.html")

                        def render_fixed():
                            current = (
                                engine.get_template("page.html")
                                if render_path == "load_and_render"
                                else template
                            )
                            return current.render(Context({}))

                        for _ in range(3):
                            assert render_fixed() == "content"
                        samples = timeit.repeat(
                            render_fixed,
                            number=max(args.number, 1000),
                            repeat=args.repeat,
                        )
                        fixed_results[
                            f"debug={debug}/cached={cached}/{shape}/{name}/{render_path}"
                        ] = statistics.median(samples) / max(args.number, 1000) * 1e6
    report = {
        "python": platform.python_version(),
        "django": django.get_version(),
        "platform": platform.platform(),
        "number": args.number,
        "repeat": args.repeat,
        "iterations": args.iterations,
        "microseconds_per_iteration": results,
        "microseconds_per_render": fixed_results,
    }
    if args.json_output:
        args.json_output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        for field in ["python", "django", "number", "repeat", "iterations"]:
            if baseline[field] != report[field]:
                raise SystemExit(f"Incompatible baseline: {field}")
        failures = []
        for key, value in {**results, **fixed_results}.items():
            if "/rusty/" not in key:
                continue
            metric = (
                "microseconds_per_iteration"
                if key in results
                else "microseconds_per_render"
            )
            measurements = report[metric]
            reference = baseline[metric][key]
            # The unchanged Django arm calibrates CPU drift between builds.
            django_key = key.replace("/rusty/", "/django/")
            control_ratio = measurements[django_key] / baseline[metric][django_key]
            ratio = (value / reference) / control_ratio
            if ratio > 1 + args.max_regression:
                failures.append(f"{key}: {ratio:.2f}x baseline relative to Django")
        if failures:
            raise SystemExit("Performance regression:\n" + "\n".join(failures))


if __name__ == "__main__":
    main()
