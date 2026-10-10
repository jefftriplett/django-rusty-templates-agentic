"""Compare identical-reference conformance results for CI summaries."""

import argparse
import json
from pathlib import Path


def compare_results(base, branch):
    for field in ("django_series", "reference_commit", "tests"):
        if base[field] != branch[field]:
            raise ValueError(f"Cannot compare different {field}")
    series = ".".join(str(value) for value in branch["django_series"])
    lines = [
        f"### Django {series}",
        "",
        "| Revision | Passed / runnable | Failures | Errors | Skips | Pass % |",
        "|---|---|---|---|---|---|",
    ]
    for revision, results in (("Base", base), ("PR", branch)):
        summary = results["summary"]
        values = [
            summary[key]
            for key in ("passed", "runnable", "failures", "errors", "skipped")
        ]
        if any(type(value) is not int or value < 0 for value in values):
            raise ValueError("Invalid result counts")
        passed, runnable, failures, errors, skipped = values
        if passed + failures + errors != runnable or not runnable:
            raise ValueError("Inconsistent or empty runnable results")
        percentage = 100 * passed / runnable
        lines.append(
            f"| {revision} | {passed}/{runnable} | {failures} | {errors} | {skipped} | {percentage:.2f}% |"
        )
    commit = branch["reference_commit"]
    if len(commit) != 40 or any(char not in "0123456789abcdef" for char in commit):
        raise ValueError("Invalid reference commit")
    lines.extend(
        [
            "",
            f"Identical Django reference: `{commit}`. Skips are excluded from the denominator.",
            "",
        ]
    )
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", type=Path)
    parser.add_argument("branch", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.write_text(
        compare_results(
            json.loads(args.base.read_text()), json.loads(args.branch.read_text())
        )
    )


if __name__ == "__main__":
    main()
