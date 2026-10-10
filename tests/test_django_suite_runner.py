from scripts.run_django_test_suite import summarize_results


def test_conformance_summary_counts_failing_subtest_parents_once():
    results = {
        "passed": ["passed", "passed"],
        "failures": ["mixed"],
        "errors": [],
        "skipped": ["skipped"],
        "subtests": [
            {"parent": "failed", "outcome": "failures"},
            {"parent": "failed", "outcome": "failures"},
            {"parent": "mixed", "outcome": "errors"},
        ],
    }
    summary, formatted = summarize_results(results)
    assert summary == {
        "passed": 1,
        "failures": 1,
        "errors": 1,
        "skipped": 1,
        "runnable": 3,
        "total": 4,
        "pass_percentage": 100 / 3,
    }
    assert formatted.count("failed ... FAIL") == 1
    assert "mixed ... ERROR" in formatted
    assert "33.33%" in formatted


def test_conformance_summary_with_no_runnable_tests():
    summary, _ = summarize_results(
        {
            "passed": [],
            "failures": [],
            "errors": [],
            "skipped": ["skipped"],
            "subtests": [],
        }
    )
    assert summary["runnable"] == 0
    assert summary["pass_percentage"] == 0
