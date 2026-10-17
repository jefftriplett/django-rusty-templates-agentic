"""Measure a realistic Django page rendered through both template backends.

Use a release extension. The page extends a base template, includes 25 cards,
and uses Django User objects, model-style methods, a manager-like `.all`, i18n
tags and common filters. Templates load through cached filesystem loaders and
render with a request and seven context processors, or without a request.
Timings are warm renders, not HTTP request measurements.
Each case runs in fresh processes and checks output against Django first;
Django and Rusty remain paired within each process.
"""

import argparse
import datetime
import hashlib
import importlib.metadata
import json
import platform
import random
import statistics
import subprocess
import sys
import tempfile
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
CONTEXT_PROCESSORS = [
    "django.template.context_processors.debug",
    "django.template.context_processors.request",
    "django.template.context_processors.i18n",
    "django.template.context_processors.tz",
    "django.template.context_processors.static",
    "django.contrib.auth.context_processors.auth",
    "django.contrib.messages.context_processors.messages",
]


def configure():
    import django
    from django.conf import settings

    options = {
        "context_processors": CONTEXT_PROCESSORS,
        "loaders": [
            (
                "django.template.loaders.cached.Loader",
                [("django.template.loaders.filesystem.Loader", [str(TEMPLATES)])],
            )
        ],
    }
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
                "OPTIONS": options,
            },
            {
                "BACKEND": "django_rusty_templates.RustyTemplates",
                "NAME": "rusty",
                "OPTIONS": options,
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


def measure_case(case, number, repeat):
    import django
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
    medians, paired = measure_pair(
        {
            arm: (
                lambda template=template: template.render(context, request),
                number,
                1,
            )
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
        "minimum_duration": MINIMUM_DURATION,
        "microseconds": {f"{case}/{arm}": median for arm, median in medians.items()},
        "paired_ratios": {case: paired},
    }


def isolated_report(number, repeat, process_repeats):
    """Run every case in fresh processes and take medians across them."""
    combined = None
    fixtures = {}
    with tempfile.TemporaryDirectory(prefix="rusty-realistic-page-") as directory:
        for case in CASES:
            samples = []
            for process in range(process_repeats):
                output = Path(directory, f"{case}-{process}.json")
                subprocess.run(
                    [
                        sys.executable,
                        str(Path(__file__).resolve()),
                        "--only-case",
                        case,
                        "--number",
                        str(number),
                        "--repeat",
                        str(repeat),
                        "--json-output",
                        str(output),
                    ],
                    check=True,
                    stdout=subprocess.DEVNULL,
                )
                samples.append(json.loads(output.read_text()))
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
    combined["fixture_sha256"] = hashlib.sha256(
        json.dumps(fixtures, sort_keys=True).encode()
    ).hexdigest()
    combined["process_repeats"] = process_repeats
    return combined


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=20)
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--process-repeats", type=int, default=3)
    parser.add_argument("--json-output", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--max-regression", type=float, default=0.15)
    parser.add_argument("--only-case", choices=CASES, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if min(args.number, args.repeat, args.process_repeats) <= 0:
        parser.error("number, repeat and process-repeats must be positive")
    if args.only_case:
        report = measure_case(args.only_case, args.number, args.repeat)
    else:
        report = isolated_report(args.number, args.repeat, args.process_repeats)
    if args.json_output:
        args.json_output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        for field in (
            "python",
            "django",
            "fixture_sha256",
            "number",
            "repeat",
            "isolation",
            "process_repeats",
        ):
            if baseline.get(field) != report[field]:
                raise SystemExit(f"Incompatible baseline: {field}")
        compare_pairs(baseline, report, args.max_regression)


if __name__ == "__main__":
    main()
