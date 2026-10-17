"""Substitute engines and record outcomes without editing Django's test suite."""

import atexit
import json
import os
import runpy
import sys
import unittest
from pathlib import Path

checkout = Path(sys.argv.pop(1)).resolve()
sys.path[:0] = [str(checkout / "tests"), str(checkout)]

import django

from scripts.django_test_adapter import install

install()

results = {
    "django_version": django.get_version(),
    "django_series": list(django.VERSION[:2]),
    "python_version": sys.version,
    "passed": [],
    "errors": [],
    "failures": [],
    "skipped": [],
    "subtests": [],
    "details": [],
}


@atexit.register
def save_results():
    Path(os.environ["RUSTY_AUDIT_JSON"]).write_text(json.dumps(results, indent=2))


for method, key in [
    ("addSuccess", "passed"),
    ("addError", "errors"),
    ("addFailure", "failures"),
    ("addSkip", "skipped"),
]:
    original = getattr(unittest.TextTestResult, method)

    def record(self, test, *args, original=original, key=key):
        results[key].append(test.id())
        if key in ("errors", "failures"):
            results["details"].append(
                {
                    "id": test.id(),
                    "outcome": key,
                    "traceback": self._exc_info_to_string(args[0], test),
                }
            )
        elif key == "skipped":
            results["details"].append(
                {"id": test.id(), "outcome": key, "reason": args[0]}
            )
        return original(self, test, *args)

    setattr(unittest.TextTestResult, method, record)

original_subtest = unittest.TextTestResult.addSubTest


def record_subtest(self, test, subtest, err):
    if err is not None:
        results["subtests"].append(
            {
                "parent": test.id(),
                "test": subtest.id(),
                "outcome": "failures"
                if issubclass(err[0], test.failureException)
                else "errors",
                "traceback": self._exc_info_to_string(err, test),
            }
        )
    return original_subtest(self, test, subtest, err)


unittest.TextTestResult.addSubTest = record_subtest
runpy.run_path(str(checkout / "tests" / "runtests.py"), run_name="__main__")
