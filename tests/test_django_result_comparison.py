from copy import deepcopy

import pytest

from scripts.compare_django_results import compare_results


@pytest.fixture
def reference_result():
    return {
        "django_series": [6, 2],
        "reference_commit": "a" * 40,
        "tests": ["template_tests", "template_loader"],
        "summary": {
            "passed": 1571,
            "runnable": 1574,
            "failures": 3,
            "errors": 0,
            "skipped": 14,
        },
    }


def test_report_shows_improvement_without_counting_skips(reference_result):
    branch = deepcopy(reference_result)
    branch["summary"].update(passed=1574, failures=0)
    report = compare_results(reference_result, branch)
    assert "| Base | 1571/1574 | 3 | 0 | 14 | 99.81% |" in report
    assert "| PR | 1574/1574 | 0 | 0 | 14 | 100.00% |" in report


@pytest.mark.parametrize(
    "field,value",
    [
        ("reference_commit", "b" * 40),
        ("django_series", [6, 1]),
        ("tests", ["template_tests"]),
    ],
)
def test_rejects_comparisons_against_different_test_references(
    reference_result, field, value
):
    branch = deepcopy(reference_result)
    branch[field] = value
    with pytest.raises(ValueError, match="Cannot compare different"):
        compare_results(reference_result, branch)


@pytest.mark.parametrize(
    "summary",
    [
        {"passed": 0, "runnable": 0, "failures": 0, "errors": 0, "skipped": 14},
        {"passed": 1574, "runnable": 1574, "failures": 3, "errors": 0, "skipped": 14},
    ],
)
def test_rejects_empty_or_inconsistent_results(reference_result, summary):
    branch = deepcopy(reference_result)
    branch["summary"] = summary
    with pytest.raises(ValueError, match="Inconsistent or empty"):
        compare_results(reference_result, branch)
