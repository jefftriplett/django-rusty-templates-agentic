"""Measure a realistic Django page rendered through both template backends.

Use a release extension. The page extends a base template, includes 25 cards,
and uses Django User objects, model-style methods, a manager-like `.all`, i18n
tags and common filters. Templates load through cached filesystem loaders and
render with a request and seven context processors, or without a request.
Timings are warm renders, not HTTP request measurements; automatic GC stays
enabled while timing, as in benchmark_memory_ownership.py.
Each case runs in fresh processes and checks output against Django first;
Django and Rusty remain paired within each process.

A separate fresh process then checks memory for the request case with
tracemalloc after gc.collect(). It reports traced growth across windows of
repeated renders, after one warm-up window, and the bytes and GC-tracked
objects retained after discarding engines that rendered the page; surviving
objects fail the run.
tracemalloc sees Python allocations, not memory allocated by Rust itself.
"""

import argparse
import datetime
import gc
import hashlib
import importlib.metadata
import json
import os
import platform
import random
import statistics
import subprocess
import sys
import tempfile
import tracemalloc
from pathlib import Path

if __package__:
    from .benchmark_pairs import (
        MEASUREMENT,
        MINIMUM_DURATION,
        compare_pairs,
        measure_pair,
    )
else:
    from benchmark_pairs import (
        MEASUREMENT,
        MINIMUM_DURATION,
        compare_pairs,
        measure_pair,
    )


TEMPLATES = Path(__file__).resolve().parent / "realistic_page"
CASES = ("request", "no_request")
ISOLATION = "fresh-process-per-case-v2"
PER_CASE_FIELDS = ("fixture_sha256", "microseconds", "paired_ratios")
MEMORY_CASE = "request"
MEMORY_WINDOWS = 4
WARMUP_ENGINES = 20
DISCARDED_ENGINES = 20
CONTEXT_PROCESSORS = [
    "django.template.context_processors.debug",
    "django.template.context_processors.request",
    "django.template.context_processors.i18n",
    "django.template.context_processors.tz",
    "django.template.context_processors.static",
    "django.contrib.auth.context_processors.auth",
    "django.contrib.messages.context_processors.messages",
]


def backend_options():
    return {
        "context_processors": CONTEXT_PROCESSORS,
        "loaders": [
            (
                "django.template.loaders.cached.Loader",
                [("django.template.loaders.filesystem.Loader", [str(TEMPLATES)])],
            )
        ],
    }


def configure():
    import django
    from django.conf import settings

    settings.configure(
        DEBUG=False,
        USE_I18N=True,
        USE_TZ=True,
        TIME_ZONE="UTC",
        SECRET_KEY="realistic-page-benchmark",
        STATIC_URL="/static/",
        INSTALLED_APPS=[
            "django.contrib.auth",
            "django.contrib.contenttypes",
            "django.contrib.messages",
        ],
        TEMPLATES=[
            {
                "BACKEND": "django.template.backends.django.DjangoTemplates",
                "NAME": "django",
                "OPTIONS": backend_options(),
            },
            {
                "BACKEND": "django_rusty_templates.RustyTemplates",
                "NAME": "rusty",
                "OPTIONS": backend_options(),
            },
        ],
    )
    django.setup()


class Tag:
    def __init__(self, number):
        self.name = f"Tag{number}"
        self.slug = f"tag-{number}"


class TagManager:
    def __init__(self, tags):
        self._tags = tags

    def all(self):
        return list(self._tags)


class Post:
    def __init__(self, number, author):
        self.pk = number
        self.title = (
            f"Post number {number}: a <fairly> long & descriptive title "
            "that goes on and on"
        )
        self.author = author
        self.published = datetime.datetime(
            2024, 1 + number % 12, 1 + number % 28, 12, 30, tzinfo=datetime.UTC
        )
        self.comment_count = number % 4
        self.rating = number * 0.37
        self.is_featured = number % 7 == 0
        self.summary = "" if number % 2 else f"Short summary {number}"
        self.body = "Lorem ipsum dolor sit amet, consectetur adipiscing elit " * 6
        self.tags = TagManager([Tag(tag) for tag in range(number % 4)])

    @property
    def tag_names(self):
        return [tag.name for tag in self.tags.all()]

    def get_absolute_url(self):
        return f"/posts/{self.pk}/"


class Page:
    number = 2
    num_pages = 9

    def has_next(self):
        return True

    def next_page_number(self):
        return 3


def page_context():
    from django.contrib.auth.models import User

    authors = [
        User(
            username=f"user{i}",
            first_name="Ann" if i % 2 else "",
            last_name="Lee" if i % 2 else "",
        )
        for i in range(5)
    ]
    context = {
        "site_name": "Example",
        "nav": [{"href": f"/s{i}/", "label": f"Section {i}"} for i in range(6)],
        "category": {
            "name": "news & updates",
            "description": "All the latest news from our team " * 5,
        },
        "posts": [Post(i, authors[i % 5]) for i in range(25)],
        "page": Page(),
        "stats": [
            {
                "label": f"row {i}",
                "count": i * 1000 + 7,
                "total": i * 12.5,
                "updated": datetime.datetime(2024, 5, 1, 10, i, tzinfo=datetime.UTC),
            }
            for i in range(20)
        ],
    }
    return context, authors[1]


def page_request(user):
    from django.contrib import messages
    from django.contrib.messages.storage.fallback import FallbackStorage
    from django.test import RequestFactory

    request = RequestFactory().get("/s1/")
    request.user = user
    request.session = {}
    request._messages = FallbackStorage(request)
    messages.info(request, "Saved <draft> & published")
    return request


def load_case(case):
    from django.template import engines

    configure()
    context, user = page_context()
    request = page_request(user) if case == "request" else None
    templates = {
        arm: engines[arm].get_template("post_list.html") for arm in ("django", "rusty")
    }
    expected = templates["django"].render(context, request)
    if templates["rusty"].render(context, request) != expected:
        raise SystemExit(f"Output mismatch: {case}")
    sources = {path.name: path.read_text() for path in sorted(TEMPLATES.glob("*.html"))}
    fixture_sha256 = hashlib.sha256(
        json.dumps({"sources": sources, "output": expected}, sort_keys=True).encode()
    ).hexdigest()
    return templates, context, request, expected, fixture_sha256


def measure_case(case, number, repeat):
    import django

    templates, context, request, _, fixture_sha256 = load_case(case)

    def render(template):
        # timeit disables automatic GC; native ownership is GC-visible, so
        # collections triggered by rendering belong in the measurement.
        gc.enable()
        template.render(context, request)

    medians, paired = measure_pair(
        {
            arm: (lambda template=template: render(template), number, 1)
            for arm, template in templates.items()
        },
        repeat,
        random.Random(0),
    )
    return {
        "python": platform.python_version(),
        "platform": platform.platform(),
        "django": django.get_version(),
        "rusty_version": importlib.metadata.version("django-rusty-templates-agentic"),
        "fixture_sha256": fixture_sha256,
        "number": number,
        "repeat": repeat,
        "measurement": MEASUREMENT,
        "isolation": ISOLATION,
        "gc_during_timing": "enabled",
        "minimum_duration": MINIMUM_DURATION,
        "microseconds": {f"{case}/{arm}": median for arm, median in medians.items()},
        "paired_ratios": {case: paired},
    }


def traced_bytes():
    gc.collect()
    return tracemalloc.get_traced_memory()[0]


def render_growth(template, context, request, renders):
    """Traced growth per window of renders, after one warm-up window."""
    tracemalloc.start()
    try:
        samples = []
        for _ in range(MEMORY_WINDOWS + 1):
            for _ in range(renders):
                template.render(context, request)
            samples.append(traced_bytes())
    finally:
        tracemalloc.stop()
    return [after - before for before, after in zip(samples, samples[1:])]


def discarded_engines(backend_class, context, request, expected):
    """Bytes and GC-tracked objects left behind by discarded engines."""

    def render_discarded(count):
        for index in range(count):
            backend = backend_class(
                {
                    "NAME": f"discarded-{index}",
                    "DIRS": [],
                    "APP_DIRS": False,
                    "OPTIONS": backend_options(),
                }
            )
            output = backend.get_template("post_list.html").render(context, request)
            if output != expected:
                raise SystemExit(f"Output mismatch: {backend_class.__name__}")

    tracemalloc.start()
    try:
        # Interpreter free lists and caches fill while tracing; warm them first.
        render_discarded(WARMUP_ENGINES)
        before = traced_bytes()
        objects = len(gc.get_objects())
        render_discarded(DISCARDED_ENGINES)
        retained = traced_bytes() - before
        surviving = len(gc.get_objects()) - objects
    finally:
        tracemalloc.stop()
    return {
        "retained_bytes_per_engine": retained / DISCARDED_ENGINES,
        "surviving_gc_objects": surviving,
    }


def measure_memory(case, renders):
    from django.template import engines

    templates, context, request, expected, fixture_sha256 = load_case(case)
    for template in templates.values():
        for _ in range(20):
            template.render(context, request)
    memory = {
        "render_growth_bytes": {
            arm: render_growth(template, context, request, renders)
            for arm, template in templates.items()
        },
        "discarded_engines": {
            arm: discarded_engines(type(engines[arm]), context, request, expected)
            for arm, template in templates.items()
        },
    }
    return {"fixture_sha256": fixture_sha256, "memory": memory}


def run_child(case, arguments, output):
    # Match benchmark_memory_ownership.py's worker environment.
    env = {**os.environ, "PYTHONHASHSEED": "0", "PYTHONNOUSERSITE": "1"}
    env.pop("PYTHONPATH", None)
    subprocess.run(
        [
            sys.executable,
            str(Path(__file__).resolve()),
            "--only-case",
            case,
            *arguments,
            "--json-output",
            str(output),
        ],
        check=True,
        env=env,
        stdout=subprocess.DEVNULL,
    )
    return json.loads(output.read_text())


def isolated_report(number, repeat, process_repeats, memory_renders):
    """Run every case in fresh processes and take medians across them."""
    combined = None
    fixtures = {}
    with tempfile.TemporaryDirectory(prefix="rusty-realistic-page-") as directory:
        for case in CASES:
            samples = [
                run_child(
                    case,
                    ["--number", str(number), "--repeat", str(repeat)],
                    Path(directory, f"{case}-{process}.json"),
                )
                for process in range(process_repeats)
            ]
            if len({sample["fixture_sha256"] for sample in samples}) != 1:
                raise ValueError(f"Incompatible child report: {case} fixture_sha256")
            fixtures[case] = samples[0]["fixture_sha256"]
            for sample in samples:
                shared = {
                    field: value
                    for field, value in sample.items()
                    if field not in PER_CASE_FIELDS
                }
                if combined is None:
                    combined = {**shared, "microseconds": {}, "paired_ratios": {}}
                for field, value in shared.items():
                    if combined[field] != value:
                        raise ValueError(f"Incompatible child report: {field}")
            combined["microseconds"].update(
                {
                    name: statistics.median(
                        sample["microseconds"][name] for sample in samples
                    )
                    for name in samples[0]["microseconds"]
                }
            )
            combined["paired_ratios"][case] = statistics.median(
                sample["paired_ratios"][case] for sample in samples
            )
        memory = run_child(
            MEMORY_CASE,
            ["--memory-renders", str(memory_renders), "--memory"],
            Path(directory, "memory.json"),
        )
    if memory["fixture_sha256"] != fixtures[MEMORY_CASE]:
        raise ValueError("Incompatible child report: memory fixture_sha256")
    combined["fixture_sha256"] = hashlib.sha256(
        json.dumps(fixtures, sort_keys=True).encode()
    ).hexdigest()
    combined["process_repeats"] = process_repeats
    combined["memory_renders"] = memory_renders
    combined["memory"] = {"case": MEMORY_CASE, **memory["memory"]}
    return combined


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=20)
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--process-repeats", type=int, default=3)
    parser.add_argument("--json-output", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--max-regression", type=float, default=0.15)
    parser.add_argument("--memory-renders", type=int, default=200)
    parser.add_argument("--only-case", choices=CASES, help=argparse.SUPPRESS)
    parser.add_argument("--memory", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if min(args.number, args.repeat, args.process_repeats, args.memory_renders) <= 0:
        parser.error(
            "number, repeat, process-repeats and memory-renders must be positive"
        )
    if args.only_case and args.memory:
        report = measure_memory(args.only_case, args.memory_renders)
    elif args.only_case:
        report = measure_case(args.only_case, args.number, args.repeat)
    else:
        report = isolated_report(
            args.number, args.repeat, args.process_repeats, args.memory_renders
        )
    if args.json_output:
        args.json_output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if args.only_case:
        return
    for arm, result in report["memory"]["discarded_engines"].items():
        if result["surviving_gc_objects"] >= DISCARDED_ENGINES:
            raise SystemExit(f"Discarded {arm} engines were not collected")
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        for field in (
            "python",
            "django",
            "fixture_sha256",
            "number",
            "repeat",
            "isolation",
            "gc_during_timing",
            "process_repeats",
        ):
            if baseline.get(field) != report[field]:
                raise SystemExit(f"Incompatible baseline: {field}")
        compare_pairs(baseline, report, args.max_regression)


if __name__ == "__main__":
    main()
