"""Isolated optimized-build regression benchmark, with natural automatic GC.

One case and release per worker; paired vanilla Django control; shuffled release
order and three independent processes; no manual collection inside timed loops.
Manual GC and fixture initialization run outside the timed region.
Run optimized builds, for example:
    python scripts/benchmark_memory_ownership.py --python old=/old/bin/python \
        --python new=/new/bin/python --result-directory /tmp/memory-benchmark
No performance thresholds run in CI. Compilation and rendering are reported
separately. Every render matches Django's output. Workers require psutil.
"""

import argparse
import gc
import hashlib
from importlib.metadata import version
import json
import os
from pathlib import Path
import random
import statistics
import subprocess
import sys
import time
from types import ModuleType


def setup(case):
    from django.conf import settings

    settings.configure(
        DEBUG=False,
        INSTALLED_APPS=[],
        USE_I18N=False,
        USE_TZ=False,
        SECRET_KEY="performance-audit",
    )
    import django

    django.setup()
    from django.template import Context, Library, Node
    from django.template.backends.django import DjangoTemplates
    from django_rusty_templates import RustyTemplates
    import django_rusty_templates.django_rusty_templates as extension

    module = ModuleType("perf_audit_tags")
    module.register = Library()
    sys.modules[module.__name__] = module

    @module.register.simple_tag
    def echo(value):
        return value

    @module.register.simple_tag(takes_context=True)
    def read(context):
        return context["value"]

    @module.register.simple_tag(takes_context=True)
    def mutate(context):
        context["changed"] = context.get("i", 1)
        return context["value"]

    @module.register.simple_tag(takes_context=True)
    def retain(context):
        context["retained"].append(context)
        return context["value"]

    @module.register.simple_block_tag(takes_context=True)
    def body(context, content):
        return content

    @module.register.inclusion_tag("card.html", takes_context=True)
    def card(context, row):
        return {"row": row}

    class Opaque(Node):
        def render(self, context):
            return str(context["value"])

    @module.register.tag("opaque")
    def opaque(parser, token):
        return Opaque()

    rows = [
        {
            "name": f"Product {i} & <details>",
            "slug": f"item-{i}",
            "price": i + 0.25,
            "available": bool(i % 3),
        }
        for i in range(100)
    ]
    values = {
        "value": "Café <&>",
        "rows": rows,
        "items": list(range(30)),
        "title": "Django & Rusty",
        "retained": [],
    }
    catalog = '<ul>{% for row in rows %}<li data-id="{{ forloop.counter }}"><a href="/{{ row.slug }}/">{{ row.name }}</a>{% if row.available %}{{ row.price|floatformat:2 }}{% endif %}</li>{% endfor %}</ul>'
    sources = {
        "literal": "<p>Hello world.</p>" * 20,
        "escaped_variables": "<h1>{{ title }}</h1><p>{{ value }}</p>" * 4,
        "loop_100": "{% for row in rows %}{{ row.name }}{% if row.available %}yes{% endif %}{% endfor %}",
        "loop_counters": "{% for row in rows %}{{ forloop.counter }}:{{ row.name }};{% endfor %}",
        "filters_100": "{% for row in rows %}{{ row.name|slugify|upper }}:{{ row.name|length }};{% endfor %}",
        "catalog_100": catalog,
        "includes_100": '{% for row in rows %}{% include "card.html" %}{% endfor %}',
        "nested_includes": '{% for i in items %}{% include "nested.html" %}{% endfor %}',
        "inheritance": '{% extends "base.html" %}{% block main %}{{ block.super }}'
        + catalog
        + "{% endblock %}",
        "with_scopes": "{% with local=value %}{{ local }}{{ value }}{% endwith %}" * 8,
        "block_scopes": "".join(
            "{% block b" + str(i) + " %}{{ value }}{% endblock %}" for i in range(8)
        ),
        "simple_tag": "{% load callbacks %}{% for i in items %}{% echo value %}{% endfor %}",
        "context_read": "{% load callbacks %}{% for i in items %}{% read %}{% endfor %}",
        "context_mutate": "{% load callbacks %}{% for i in items %}{% mutate %}{% endfor %}",
        "context_retain": "{% load callbacks %}{% for i in items %}{% retain %}{% endfor %}",
        "context_block": "{% load callbacks %}{% body %}{% for i in items %}{{ value }}{% endfor %}{% endbody %}",
        "inclusion_tags": "{% load callbacks %}{% for row in rows %}{% card row %}{% endfor %}",
        "opaque_tags": "{% load callbacks %}{% for i in items %}{% opaque %}{% endfor %}",
        "callable_flat": "{{ callback }}" * 30,
        "callable_loop": "{% for i in items %}{{ callback }}{% endfor %}",
    }
    base, _, operation = case.partition("/")
    size = 0
    for suffix in ("_1000", "_64"):
        if base.endswith(suffix) and base[: -len(suffix)] in sources:
            size = int(suffix[1:])
            base = base[: -len(suffix)]
            break
    values.update({f"padding_{i}": i for i in range(size)})
    templates = {
        "page.html": sources[base],
        "base.html": "<main>{% block main %}base{% endblock %}</main>",
        "card.html": "<li>{{ row.name }}:{{ row.price|floatformat:2 }}</li>",
        "nested.html": '{% for row in rows %}{% include "tiny.html" %}{% endfor %}',
        "tiny.html": "<b>{{ row.name }}</b>",
    }

    def backend(cls, cached=True):
        loaders = [("django.template.loaders.locmem.Loader", templates)]
        if cached:
            loaders = [("django.template.loaders.cached.Loader", loaders)]
        return cls(
            {
                "NAME": "benchmark",
                "DIRS": [],
                "APP_DIRS": False,
                "OPTIONS": {
                    "debug": False,
                    "libraries": {"callbacks": module.__name__},
                    "loaders": loaders,
                },
            }
        )

    backends = {
        "django": backend(DjangoTemplates, operation != "uncached"),
        "rusty": backend(RustyTemplates, operation != "uncached"),
    }
    compiled = {
        arm: engine.get_template("page.html") for arm, engine in backends.items()
    }
    funcs, outputs = {}, {}
    for arm, engine in backends.items():
        own_values = dict(values, retained=[])
        template = compiled[arm]
        if base.startswith("callable"):
            caller = Context(own_values)

            def callback(caller=caller):
                return caller["value"]

            caller["callback"] = callback
            template = getattr(template, "template", template)
            own_values = caller

        def render(template=template, own_values=own_values):
            try:
                return template.render(own_values)
            finally:
                own_values["retained"].clear()

        outputs[arm] = render()
        if operation == "compile":
            funcs[arm] = lambda engine=engine: engine.from_string(sources[base])
        elif operation in ("cached", "uncached"):
            funcs[arm] = lambda engine=engine, own_values=own_values: (
                engine.get_template("page.html").render(own_values)
            )
        else:
            funcs[arm] = render
    assert outputs["django"] == outputs["rusty"], case
    metadata = {
        "python": sys.version,
        "django": django.get_version(),
        "rusty": version("django-rusty-templates-agentic"),
        "extension": extension.__file__,
        "output_sha256": hashlib.sha256(outputs["django"].encode()).hexdigest(),
        "output_bytes": len(outputs["django"].encode()),
        "source_sha256": hashlib.sha256(sources[base].encode()).hexdigest(),
        "context_padding": size,
    }
    return funcs, metadata


CASES = [
    name + "/render"
    for name in (
        "literal",
        "escaped_variables",
        "loop_100",
        "loop_counters",
        "filters_100",
        "catalog_100",
        "includes_100",
        "nested_includes",
        "inheritance",
        "with_scopes",
        "with_scopes_1000",
        "block_scopes",
        "block_scopes_1000",
        "simple_tag",
        "context_read",
        "context_read_64",
        "context_read_1000",
        "context_mutate",
        "context_mutate_1000",
        "context_retain",
        "context_retain_1000",
        "context_block",
        "inclusion_tags",
        "opaque_tags",
        "callable_flat",
        "callable_flat_1000",
        "callable_loop",
        "callable_loop_1000",
    )
]
CASES += [
    "catalog_100/compile",
    "inheritance/compile",
    "opaque_tags/compile",
    "catalog_100/cached",
    "catalog_100/uncached",
]


def batch(operation, number):
    start = time.perf_counter_ns()
    for _ in range(number):
        operation()
    return (time.perf_counter_ns() - start) / number


def worker(args):
    funcs, metadata = setup(args.case)
    assert gc.isenabled()
    import psutil

    process = psutil.Process()
    samples = {arm: [] for arm in funcs}
    counts = {}
    shuffle = random.Random(args.seed)
    for arm, operation in funcs.items():
        for _ in range(20):
            operation()
        gc.collect()
        number = 1
        while True:
            elapsed = batch(operation, number) * number
            if elapsed >= args.target_ms * 1e6:
                break
            number = min(
                100000,
                max(number + 1, int(number * args.target_ms * 1e6 / max(elapsed, 1))),
            )
        counts[arm] = number
    gc.collect()
    before = process.memory_full_info().uss
    for _ in range(args.repeat):
        order = list(funcs)
        shuffle.shuffle(order)
        for arm in order:
            gc.collect()  # Excluded from timing; natural automatic GC stays enabled.
            samples[arm].append(batch(funcs[arm], counts[arm]))
    gc.collect()
    after = process.memory_full_info().uss
    report = {
        "case": args.case,
        "metadata": metadata,
        "counts": counts,
        "samples_ns": samples,
        "median_us": {arm: statistics.median(s) / 1000 for arm, s in samples.items()},
        "paired_ratio_rusty_over_django": statistics.median(
            r / d for r, d in zip(samples["rusty"], samples["django"])
        ),
        "gc_during_timing": "enabled",
        "uss_change_bytes": after - before,
        "repeat": args.repeat,
        "target_ms": args.target_ms,
    }
    print(json.dumps(report))


def run(args):
    raw = args.result_directory.resolve()
    raw.mkdir(parents=True, exist_ok=True)
    cases = args.cases.split(",") if args.cases else CASES
    pythons = (
        dict(item.split("=", 1) for item in args.python)
        if args.python
        else {"current": sys.executable}
    )
    if not all(label and executable for label, executable in pythons.items()):
        raise ValueError("--python requires a nonempty LABEL=EXECUTABLE")
    versions = list(pythons)
    jobs = [
        (repeat, case, version)
        for repeat in range(args.processes)
        for case in cases
        for version in versions
    ]
    random.Random(args.seed).shuffle(jobs)
    started = time.monotonic()
    fingerprints = {}
    for index, (repeat, case, release) in enumerate(jobs, 1):
        output = raw / f"{case.replace('/', '--')}--{release}--{repeat}.json"
        if output.exists():
            raise ValueError(
                f"Result already exists; use a fresh result directory: {output}"
            )
        python = pythons[release]
        env = {**os.environ, "PYTHONHASHSEED": "0", "PYTHONNOUSERSITE": "1"}
        env.pop("PYTHONPATH", None)
        completed = subprocess.run(
            [
                str(python),
                str(Path(__file__).resolve()),
                "--worker",
                "--case",
                case,
                "--repeat",
                str(args.repeat),
                "--target-ms",
                str(args.target_ms),
                "--seed",
                str(args.seed + repeat),
            ],
            env=env,
            capture_output=True,
            text=True,
            timeout=120,
        )
        if completed.returncode:
            (output.with_suffix(".stderr")).write_text(completed.stderr)
            raise RuntimeError(f"{case}/{release}: {completed.stderr}")
        report = json.loads(completed.stdout)
        fingerprint = {
            key: value
            for key, value in report["metadata"].items()
            if key not in ("rusty", "extension")
        }
        if fingerprint != fingerprints.setdefault(case, fingerprint):
            raise ValueError(
                f"Incompatible runtime, fixture or output: {case}/{release}"
            )
        report["process_repeat"] = repeat
        report["label"] = release
        report["loadavg_after"] = os.getloadavg() if hasattr(os, "getloadavg") else None
        output.write_text(json.dumps(report, indent=2) + "\n")
        print(
            f"{index}/{len(jobs)} {release} {case} Django={report['median_us']['django']:.2f}us Rusty={report['median_us']['rusty']:.2f}us ratio={report['paired_ratio_rusty_over_django']:.3f}",
            flush=True,
        )
    print(f"Completed in {time.monotonic() - started:.1f}s", flush=True)


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--worker", action="store_true")
parser.add_argument("--case")
parser.add_argument("--cases")
parser.add_argument("--python", action="append", default=[], metavar="LABEL=EXECUTABLE")
parser.add_argument(
    "--result-directory", type=Path, default=Path("memory-benchmark-results")
)
parser.add_argument("--processes", type=int, default=3)
parser.add_argument("--repeat", type=int, default=7)
parser.add_argument("--target-ms", type=float, default=30)
parser.add_argument("--seed", type=int, default=20261018)
args = parser.parse_args()
if min(args.processes, args.repeat, args.target_ms) <= 0:
    parser.error("processes, repeat and target-ms must be positive")
if __name__ == "__main__":
    worker(args) if args.worker else run(args)
