"""Compare Django and Rusty across the kinds of templates real projects have.

Each scenario renders through the `DjangoTemplates` and `RustyTemplates`
backends with filesystem loaders, checks that both produce the same output,
then times them alternately. Scenarios cover where Rusty is fastest (native
loops, filters, inheritance), where Python code dominates (custom tags and
filters, model methods), mixed templates that use Django's parser for a custom
tag, and loading without the cached loader.

Build a release extension first (`uv run maturin develop --release`). Results
are synthetic render timings, not HTTP request measurements; see
docs/benchmarks/what-to-expect.md for how to read them.
"""

import argparse
import datetime
import importlib.metadata
import json
import platform
import statistics
import sys
import tempfile
import timeit
from decimal import Decimal
from pathlib import Path
from types import ModuleType

import django
from django.conf import settings

ROWS = 200

TEMPLATES = {
    "tiny.html": "<h1>{{ title }}</h1><p>{{ subtitle }}</p>",
    "table.html": (
        "<table>{% for row in rows %}<tr class=\"{% cycle 'odd' 'even' %}\">"
        "<td>{{ forloop.counter }}</td><td>{{ row.name|title }}</td>"
        "<td>{{ row.email|lower }}</td><td>{{ row.joined|date:'Y-m-d' }}</td>"
        "<td>{{ row.balance|floatformat:2 }}</td><td>{{ row.note|default:'-'|truncatechars:20 }}</td>"
        "{% if row.active %}<td>yes</td>{% else %}<td>no</td>{% endif %}</tr>"
        "{% empty %}<tr><td>none</td></tr>{% endfor %}</table>"
    ),
    "base.html": (
        "<html><head><title>{% block title %}Site{% endblock %}</title></head><body>"
        "<nav>{% for link in links %}<a href='{{ link.url }}'>{{ link.label }}</a>{% endfor %}</nav>"
        "<main>{% block content %}{% endblock %}</main>"
        "<footer>{% block footer %}&copy; {{ year }}{% endblock %}</footer></body></html>"
    ),
    "card.html": "<article><h2>{{ item.name|title }}</h2><p>{{ item.note|default:'-' }}</p></article>",
    "page.html": (
        "{% extends 'base.html' %}{% block title %}{{ title }} | {{ block.super }}{% endblock %}"
        "{% block content %}{% for item in rows|slice:':20' %}"
        "{% include 'card.html' %}{% endfor %}{% endblock %}"
    ),
    "objects.html": (
        "{% for row in people %}<li>{{ row.full_name }} {{ row.get_display }} "
        "{{ row.profile.city }} {{ row.is_admin|yesno:'admin,user' }}</li>{% endfor %}"
    ),
    "simple_tags.html": (
        "{% load bench %}{% for row in rows %}<li>{% badge row.name %}</li>{% endfor %}"
    ),
    "custom_filters.html": (
        "{% load bench %}{% for row in rows %}<li>{{ row.name|shout }}</li>{% endfor %}"
    ),
    "urlize.html": "{% for row in rows %}<p>{{ row.email|urlize }}</p>{% endfor %}",
    "mixed.html": (
        "{% load bench %}{% opaque %}<table>{% for row in rows %}<tr>"
        "<td>{{ row.name|title }}</td><td>{{ row.email|lower }}</td>"
        "<td>{{ row.balance|floatformat:2 }}</td></tr>{% endfor %}</table>"
    ),
}

SCENARIOS = [
    # name, template, shows
    ("tiny template", "tiny.html", "per-render overhead"),
    ("table with filters", "table.html", f"{ROWS} rows of native filters"),
    ("inheritance and includes", "page.html", "extends, blocks, 20 includes"),
    ("model-style objects", "objects.html", "Python properties and methods"),
    ("tiny template, 30 context keys", "tiny.html", "context conversion"),
    ("tiny template, 100 context keys", "tiny.html", "context conversion"),
    ("tiny template, 500 context keys", "tiny.html", "context conversion"),
    ("urlize on email addresses", "urlize.html", "words handed to Django's urlizer"),
    ("custom simple_tag per row", "simple_tags.html", "Python tag calls"),
    ("custom filter per row", "custom_filters.html", "Python filter calls"),
    ("mixed template", "mixed.html", "one custom Node tag, rest native"),
]


def bench_library():
    from django.template import Library, Node

    library = Library()

    @library.simple_tag
    def badge(name):
        return f"[{name}]"

    @library.filter
    def shout(value):
        return f"{value}!"

    class OpaqueNode(Node):
        def render(self, context):
            return "<!-- opaque -->"

    @library.tag
    def opaque(parser, token):
        return OpaqueNode()

    module = ModuleType("benchmark_summary_tags")
    module.register = library
    sys.modules[module.__name__] = module
    return {"bench": module.__name__}


class Profile:
    def __init__(self, city):
        self.city = city


class Person:
    def __init__(self, index):
        self.first = f"first{index}"
        self.last = f"last{index}"
        self.profile = Profile(f"city{index % 7}")
        self.is_admin = index % 5 == 0

    @property
    def full_name(self):
        return f"{self.first} {self.last}"

    def get_display(self):
        return self.full_name.upper()


def context():
    rows = [
        {
            "name": f"person {i}",
            "email": f"Person{i}@Example.com",
            "joined": datetime.date(2020, 1, 1) + datetime.timedelta(days=i),
            "balance": Decimal(i) * Decimal("10.125"),
            "note": "" if i % 3 else f"note about person {i} that is long",
            "active": i % 2 == 0,
        }
        for i in range(ROWS)
    ]
    return {
        "title": "Dashboard",
        "subtitle": "<b>Welcome</b>",
        "rows": rows,
        "people": [Person(i) for i in range(ROWS)],
        "links": [{"url": f"/{i}/", "label": f"Link {i}"} for i in range(8)],
        "year": 2026,
    }


def with_keys(values, label):
    """The scenario's context, padded to its stated number of keys."""
    if "context keys" not in label:
        return values
    keys = int(label.split(", ")[1].split()[0])
    padded = {"title": values["title"], "subtitle": values["subtitle"]}
    padded.update((f"setting_{i}", i) for i in range(keys - len(padded)))
    return padded


def backends(directory, libraries, debug, cached):
    from django.template.backends.django import DjangoTemplates

    from django_rusty_templates import RustyTemplates

    loader = ("django.template.loaders.filesystem.Loader", [str(directory)])
    loaders = (
        [("django.template.loaders.cached.Loader", [loader])] if cached else [loader]
    )
    result = {}
    for name, cls in [("django", DjangoTemplates), ("rusty", RustyTemplates)]:
        result[name] = cls(
            {
                "NAME": name,
                "DIRS": [],
                "APP_DIRS": False,
                "OPTIONS": {"debug": debug, "loaders": loaders, "libraries": libraries},
            }
        )
    return result


def measure(functions, number, repeat):
    """Alternate the engines so drift affects both; keep each one's best."""
    best = {name: float("inf") for name in functions}
    for _ in range(repeat):
        for name, function in functions.items():
            seconds = timeit.timeit(function, number=number) / number
            best[name] = min(best[name], seconds)
    return {name: seconds * 1e6 for name, seconds in best.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=50)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--json-output", type=Path)
    args = parser.parse_args()
    if not settings.configured:
        settings.configure(USE_TZ=False, USE_I18N=False)
    django.setup()
    libraries = bench_library()
    values = context()
    results = []
    with tempfile.TemporaryDirectory(prefix="rusty-summary-") as directory:
        for name, source in TEMPLATES.items():
            Path(directory, name).write_text(source)
        modes = [
            ("warm, cached loader", False, True, SCENARIOS),
            (
                "load and render, no cached loader",
                False,
                False,
                [
                    s
                    for s in SCENARIOS
                    if s[1] in ("table.html", "page.html", "mixed.html")
                ],
            ),
            (
                "compile only, no cached loader",
                False,
                False,
                [
                    s
                    for s in SCENARIOS
                    if s[1] in ("tiny.html", "table.html", "page.html", "mixed.html")
                    and "context keys" not in s[0]
                ],
            ),
            (
                "warm, debug=True",
                True,
                True,
                [s for s in SCENARIOS if s[1] in ("table.html", "mixed.html")],
            ),
        ]
        for mode, debug, cached, scenarios in modes:
            engines = backends(directory, libraries, debug, cached)
            for label, template_name, shows in scenarios:
                data = with_keys(values, label)
                if mode.startswith("compile only"):
                    source = TEMPLATES[template_name]
                    functions = {
                        name: (lambda e=engine: e.from_string(source))
                        for name, engine in engines.items()
                    }
                elif cached:
                    templates = {
                        name: engine.get_template(template_name)
                        for name, engine in engines.items()
                    }
                    functions = {
                        name: (lambda t=template: t.render(data))
                        for name, template in templates.items()
                    }
                else:
                    functions = {
                        name: (
                            lambda e=engine: e.get_template(template_name).render(data)
                        )
                        for name, engine in engines.items()
                    }
                outputs = {name: function() for name, function in functions.items()}
                if mode.startswith("compile only"):
                    outputs = {
                        name: template.render(data)
                        for name, template in outputs.items()
                    }
                if outputs["django"] != outputs["rusty"]:
                    raise SystemExit(f"{label} ({mode}): outputs differ")
                timings = measure(functions, args.number, args.repeat)
                results.append(
                    {
                        "mode": mode,
                        "scenario": label,
                        "shows": shows,
                        "django_us": round(timings["django"], 1),
                        "rusty_us": round(timings["rusty"], 1),
                        "speedup": round(timings["django"] / timings["rusty"], 2),
                    }
                )
                row = results[-1]
                print(
                    f"{mode:36} {label:32} django {row['django_us']:9.1f} µs  "
                    f"rusty {row['rusty_us']:9.1f} µs  {row['speedup']:5.2f}×",
                    flush=True,
                )
    if args.json_output:
        args.json_output.write_text(
            json.dumps(
                {
                    "python": platform.python_version(),
                    "platform": platform.platform(),
                    "django": django.get_version(),
                    "rusty_version": importlib.metadata.version(
                        "django-rusty-templates-agentic"
                    ),
                    "rows": ROWS,
                    "number": args.number,
                    "repeat": args.repeat,
                    "results": results,
                    "median_warm_speedup": statistics.median(
                        r["speedup"]
                        for r in results
                        if r["mode"] == "warm, cached loader"
                    ),
                },
                indent=2,
            )
            + "\n"
        )


if __name__ == "__main__":
    main()
