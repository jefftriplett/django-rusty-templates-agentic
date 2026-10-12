"""Compare native inheritance compilation and rendering with Django/hybrid.

Run after `maturin develop --release`:
    .venv/bin/python scripts/benchmark_inheritance.py --number 200 --repeat 5

The fixture is synthetic, not an application request benchmark. All timed
render paths must first produce identical output. Compilation is measured
separately from warm rendering and uncached loading plus first rendering.
"""

import argparse
import json
import platform
import statistics
import timeit

import django
from django.conf import settings
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine
from django_rusty_templates.interop import compile_template


def fixture():
    names = [f"section_{index}" for index in range(8)]
    base = (
        "<html>"
        + "".join(
            f"{{% block {name} %}}base {{{{ title }}}}{{% endblock %}}"
            for name in names
        )
        + "</html>"
    )
    row = "<p>{{ row.name }} / {{ row.code }} / {{ row.price|floatformat:2 }}</p>"
    body = "<h2>{{ title }} / {{ subtitle }}</h2>" + row * 24
    child = '{% extends "base.html" %}' + "".join(
        f"{{% block {name} %}}{{{{ block.super }}}}"
        + "{% for row in rows %}"
        + body
        + "{% endfor %}{% endblock %}"
        for name in names
    )
    return {"base.html": base, "page.html": child}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=200)
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--json-output")
    args = parser.parse_args()
    if not settings.configured:
        settings.configure(USE_I18N=False, SECRET_KEY="benchmark")
    templates = fixture()
    source = templates["page.html"]
    values = {
        "title": "Catalog <&>",
        "subtitle": "Café",
        "rows": [
            {"name": f"item-{index}", "code": "<&>", "price": index + 0.25}
            for index in range(5)
        ],
    }
    results = {
        "python": platform.python_version(),
        "django": django.get_version(),
        "platform": platform.platform(),
        "number": args.number,
        "repeat": args.repeat,
        "source_bytes": len(source.encode()),
        "timings_us": {},
    }
    for debug in (False, True):
        loaders = [("django.template.loaders.locmem.Loader", templates)]
        django_engine = DjangoEngine(loaders=loaders, debug=debug)
        rusty_engine = Engine(loaders=loaders, debug=debug)
        django_template = django_engine.get_template("page.html")
        native_template = rusty_engine.get_template("page.html")
        hybrid_template = compile_template(
            source, rusty_engine, "page.html", "page.html"
        )
        expected = django_template.render(Context(values))
        assert native_template._is_native
        assert native_template.render(Context(values)) == expected
        assert hybrid_template.render(Context(values)) == expected
        cases = {
            "compile_django": lambda: django_engine.from_string(source),
            "compile_native": lambda: rusty_engine.from_string(source),
            "compile_hybrid": lambda: compile_template(source, rusty_engine),
            "render_django": lambda: django_template.render(Context(values)),
            "render_native": lambda: native_template.render(Context(values)),
            "render_hybrid": lambda: hybrid_template.render(Context(values)),
            "load_compile_render_django": lambda: django_engine.get_template(
                "page.html"
            ).render(Context(values)),
            "load_compile_render_native": lambda: rusty_engine.get_template(
                "page.html"
            ).render(Context(values)),
        }
        timings = {}
        for name, function in cases.items():
            samples = timeit.repeat(function, number=args.number, repeat=args.repeat)
            timings[name] = statistics.median(samples) / args.number * 1_000_000
        results["timings_us"][f"debug={debug}"] = timings
        print(f"DEBUG={debug}, source={len(source.encode())} bytes")
        for operation in ("compile", "render", "load_compile_render"):
            baseline = timings[f"{operation}_django"]
            native = timings[f"{operation}_native"]
            print(
                f"  {operation:20s} Django={baseline:9.1f} us  Rusty={native:9.1f} us  speedup={baseline / native:.2f}x"
            )
        print(
            f"  hybrid compile={timings['compile_hybrid']:.1f} us; render={timings['render_hybrid']:.1f} us"
        )
    if args.json_output:
        from pathlib import Path

        Path(args.json_output).write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
