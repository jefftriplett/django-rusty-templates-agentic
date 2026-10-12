"""Paired callback-context benchmark for native and disk-loaded templates.

Build with maturin develop --release first. Results are synthetic microseconds
per callback, not HTTP latency. --baseline compares matching paired Django
controls without changing the other render benchmarks' regression threshold.
"""

import argparse
import json
import platform
import random
import statistics
import tempfile
import timeit
from pathlib import Path

import django
from django.conf import settings
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine


def render_case(engine, source, filename, path, mode, size, iterations, shape):
    template = (
        engine.from_string(source)
        if path == "string"
        else engine.get_template(filename)
    )
    context = Context(
        {"items": list(range(iterations)), **{f"padding_{i}": i for i in range(size)}}
    )
    plain = shape.startswith("plain_")
    if plain:
        context["value"] = "value" if mode == "string" else 123
    expected = str(context["value"]) * source.count("{{") if plain else ""
    retained = []
    error = RuntimeError("benchmark callback")
    counter = 0

    def callback():
        nonlocal counter
        value = context["i"] if shape == "scoped" else counter
        counter += 1
        if mode == "mutate":
            context["changed"] = value
        elif mode == "retain":
            retained.append(context.dicts[-1])
        elif mode == "exception" and value == iterations - 1:
            raise error
        return ""

    context["callback"] = callback
    depth = len(context.dicts)

    def render():
        nonlocal counter, error
        counter = 0
        if mode == "exception":
            error = RuntimeError("benchmark callback")
        current = (
            engine.get_template(filename) if path == "load_and_render" else template
        )
        try:
            output = current.render(context)
        except RuntimeError as caught:
            assert mode == "exception" and caught is error
            # Reusing the same original error must not grow its traceback with
            # every timed render and distort subsequent measurements.
            error.__traceback__ = None
        else:
            assert mode != "exception" and output == expected
        assert len(context.dicts) == depth
        retained.clear()

    for _ in range(3):
        render()
    return render


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=30)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--json-output", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--max-regression", type=float, default=0.15)
    args = parser.parse_args()
    if min(args.number, args.repeat, args.iterations) <= 0:
        parser.error("number, repeat and iterations must be positive")
    if not settings.configured:
        settings.configure(USE_I18N=False)
    sources = {
        "scoped": "{% for i in items %}{{ callback }}{% endfor %}",
        "flat": "{{ callback }}" * args.iterations,
        "plain_1": "{{ value }}",
        "plain_1000": "{{ value }}" * 1000,
    }
    results, plain_results, paired = {}, {}, {}
    shuffle = random.Random(133)
    with tempfile.TemporaryDirectory(prefix="rusty-callback-benchmark-") as directory:
        for shape, source in sources.items():
            Path(directory, f"{shape}.html").write_text(source)
        for debug in (False, True):
            for cached in (False, True):
                loader = "django.template.loaders.filesystem.Loader"
                loaders = (
                    [("django.template.loaders.cached.Loader", [loader])]
                    if cached
                    else [loader]
                )
                engines = {
                    "rusty": Engine(dirs=[directory], loaders=loaders, debug=debug),
                    "django": DjangoEngine(
                        dirs=[directory], loaders=loaders, debug=debug
                    ),
                }
                for shape, source in sources.items():
                    for path in ("string", "loaded", "load_and_render"):
                        plain = shape.startswith("plain_")
                        for size in (1000,) if plain else (0, 16, 32, 64, 1000):
                            for mode in (
                                ("string", "integer")
                                if plain
                                else ("read", "mutate", "retain", "exception")
                            ):
                                key = f"debug={debug}/cached={cached}/{shape}_{size}_{mode}"
                                renders = {
                                    name: render_case(
                                        engine,
                                        source,
                                        f"{shape}.html",
                                        path,
                                        mode,
                                        size,
                                        args.iterations,
                                        shape,
                                    )
                                    for name, engine in engines.items()
                                }
                                samples = {name: [] for name in renders}
                                ratios = []
                                for _ in range(args.repeat):
                                    order = list(renders)
                                    shuffle.shuffle(order)
                                    measured = {
                                        name: timeit.timeit(
                                            renders[name], number=args.number
                                        )
                                        for name in order
                                    }
                                    for name, sample in measured.items():
                                        samples[name].append(
                                            sample
                                            / args.number
                                            / (1 if plain else args.iterations)
                                            * 1e6
                                        )
                                    ratios.append(
                                        measured["rusty"] / measured["django"]
                                    )
                                for name, values in samples.items():
                                    target = plain_results if plain else results
                                    target[f"{key}/{name}/{path}"] = statistics.median(
                                        values
                                    )
                                paired[f"{key}/{path}"] = statistics.median(ratios)
    report = {
        "python": platform.python_version(),
        "django": django.get_version(),
        "number": args.number,
        "repeat": args.repeat,
        "iterations": args.iterations,
        "microseconds_per_callback": results,
        "plain_microseconds_per_render": plain_results,
        "paired_ratio_vs_django": paired,
    }
    if args.json_output:
        args.json_output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        for field in ("python", "django", "number", "repeat", "iterations"):
            if baseline[field] != report[field]:
                raise SystemExit(f"Incompatible baseline: {field}")
        failures = [
            f"{key}: {value / baseline['paired_ratio_vs_django'][key]:.2f}x baseline relative to Django"
            for key, value in paired.items()
            if value / baseline["paired_ratio_vs_django"][key] > 1 + args.max_regression
        ]
        if failures:
            raise SystemExit("Performance regression:\n" + "\n".join(failures))


if __name__ == "__main__":
    main()
