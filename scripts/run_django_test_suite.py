"""Run unmodified Django template tests with the Rusty engine substituted."""

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
PROJECT_DIR = SCRIPT_DIR.parent


def log(message):
    print(f"==> {message}", flush=True)


def summarize_results(results):
    """Count methods once, including methods with several failing subtests."""
    errors = set(results["errors"])
    failures = set(results["failures"])
    for subtest in results["subtests"]:
        target = failures if subtest["outcome"] == "failures" else errors
        target.add(subtest["parent"])
    failures -= errors
    passed = set(results["passed"]) - errors - failures
    skipped = set(results["skipped"]) - errors - failures - passed
    runnable = len(passed | errors | failures)
    summary = {
        "passed": len(passed),
        "failures": len(failures),
        "errors": len(errors),
        "skipped": len(skipped),
        "runnable": runnable,
        "total": runnable + len(skipped),
        "pass_percentage": 100 * len(passed) / runnable if runnable else 0,
    }
    lines = [
        f"Django test suite passing: {summary['pass_percentage']:.2f}%",
        f"{len(passed)} OK / {len(failures)} FAIL / {len(errors)} ERROR / {len(skipped)} SKIP",
    ]
    outcomes = {"OK": passed, "FAIL": failures, "ERROR": errors, "SKIP": skipped}
    lines.extend(
        sorted(
            f"{test} ... {outcome}"
            for outcome, tests in outcomes.items()
            for test in tests
        )
    )
    return summary, "\n".join(lines) + "\n"


def prepare_checkout(args):
    checkout = (
        args.django_checkout or SCRIPT_DIR / f".django-{args.django_version}"
    ).resolve()
    if not checkout.exists():
        if args.django_checkout:
            raise ValueError(f"Django checkout does not exist: {checkout}")
        branch = (
            "main"
            if args.django_version == "6.2"
            else f"stable/{args.django_version}.x"
        )
        log(f"Cloning {branch} into {checkout}")
        subprocess.run(
            [
                "git",
                "clone",
                "--quiet",
                "--depth",
                "1",
                "--no-tags",
                "--branch",
                branch,
                "https://github.com/django/django.git",
                str(checkout),
            ],
            check=True,
        )
    if args.django_ref:
        if args.django_checkout:
            ref = subprocess.check_output(
                ["git", "rev-parse", args.django_ref], cwd=checkout, text=True
            ).strip()
            head = subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=checkout, text=True
            ).strip()
            if ref != head:
                raise ValueError(
                    "Explicit checkout HEAD does not match --django-ref; refusing to change it"
                )
        else:
            subprocess.run(
                ["git", "fetch", "--depth", "1", "origin", args.django_ref],
                cwd=checkout,
                check=True,
            )
            subprocess.run(
                ["git", "checkout", "--detach", "FETCH_HEAD"], cwd=checkout, check=True
            )
    if subprocess.run(
        [
            "git",
            "diff",
            "--quiet",
            "HEAD",
            "--",
            "django",
            "tests/template_tests",
            "tests/template_loader",
            "tests/runtests.py",
        ],
        cwd=checkout,
    ).returncode:
        raise ValueError(
            "Django sources or template tests have tracked modifications; use a pristine checkout"
        )
    commit = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=checkout, text=True
    ).strip()
    return checkout, commit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--django-version", choices=["5.2", "6.0", "6.1", "6.2"], default="5.2"
    )
    parser.add_argument(
        "--django-ref", help="Branch, tag, or commit to fetch into the version's cache"
    )
    parser.add_argument(
        "--django-checkout",
        type=Path,
        help="Use an existing pristine checkout without modifying it",
    )
    parser.add_argument("--parsed-output", type=Path)
    parser.add_argument(
        "--json-output",
        type=Path,
        help="Save outcomes, tracebacks, skips, versions, and reference commit",
    )
    parser.add_argument("--raw-output", type=Path)
    parser.add_argument(
        "tests", nargs="*", default=["template_tests", "template_loader"]
    )
    args = parser.parse_args()
    try:
        checkout, commit = prepare_checkout(args)
    except (ValueError, subprocess.CalledProcessError) as error:
        parser.error(str(error))
    log(f"Django {args.django_version} reference: {commit}")
    with tempfile.TemporaryDirectory(prefix="rusty-django-results-") as directory:
        results_path = Path(directory) / "results.json"
        pythonpath = [str(PROJECT_DIR / "python"), str(checkout), str(PROJECT_DIR)]
        if os.environ.get("PYTHONPATH"):
            pythonpath.append(os.environ["PYTHONPATH"])
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT_DIR / "django_test_bootstrap.py"),
                str(checkout),
                "--parallel=1",
                "-v=2",
                *args.tests,
            ],
            cwd=checkout / "tests",
            env={
                **os.environ,
                "PYTHONPATH": os.pathsep.join(pythonpath),
                "RUSTY_AUDIT_JSON": str(results_path),
            },
            capture_output=True,
            text=True,
        )
        print(result.stdout, end="")
        print(result.stderr, end="", file=sys.stderr)
        if args.raw_output:
            args.raw_output.write_text(result.stdout + result.stderr)
        if not results_path.exists():
            return result.returncode or 1
        results = json.loads(results_path.read_text())
    results.update(
        reference_commit=commit,
        checkout=str(checkout),
        tests=args.tests,
        returncode=result.returncode,
    )
    summary, formatted = summarize_results(results)
    results["summary"] = summary
    if args.json_output:
        args.json_output.write_text(json.dumps(results, indent=2) + "\n")
    if args.parsed_output:
        args.parsed_output.write_text(formatted)
    log(formatted.splitlines()[0])
    log(formatted.splitlines()[1])
    expected_series = [int(value) for value in args.django_version.split(".")]
    if results["django_series"] != expected_series:
        log(f"Unexpected Django series: {results['django_series']}")
        return 1
    if not summary["runnable"]:
        log("No runnable tests recorded")
        return result.returncode or 1
    return result.returncode or (1 if summary["errors"] or summary["failures"] else 0)


if __name__ == "__main__":
    raise SystemExit(main())
