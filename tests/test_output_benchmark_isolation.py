import argparse
import json
from pathlib import Path

import pytest

from scripts import benchmark_render_output as benchmark


def test_cases_cover_every_configuration_once():
    keys = benchmark.case_keys(benchmark.fixtures())
    assert len(keys) == len(set(keys)) == 176
    assert "debug=False/cached=False/escaped_8192/loaded" in keys
    assert "debug=True/cached=True/safe_8192/from_string" in keys


def test_each_case_starts_a_new_interpreter_and_merges_only_its_pair(monkeypatch):
    calls = []
    keys = benchmark.case_keys({"example": None})[:2]

    def run(command, **options):
        calls.append(command)
        assert options["check"] is True
        key = command[command.index("--only-case") + 1]
        output = Path(command[command.index("--json-output") + 1])
        output.write_text(
            json.dumps(
                {
                    "isolation": benchmark.ISOLATION,
                    "microseconds": {f"{key}/django": 10, f"{key}/rusty": 5},
                    "paired_ratios": {key: 0.5},
                }
            )
        )

    monkeypatch.setattr(benchmark.subprocess, "run", run)
    report = benchmark.isolated_report(keys, 20, 5, process_repeats=1)
    assert len(calls) == 2
    assert all(command[0] == benchmark.sys.executable for command in calls)
    assert all(
        command[1] == str(Path(benchmark.__file__).resolve()) for command in calls
    )
    assert set(report["paired_ratios"]) == set(keys)
    assert len(report["microseconds"]) == 4


@pytest.mark.parametrize("change", ["wrong_case", "metadata", "failure"])
def test_bad_child_reports_fail_the_benchmark(monkeypatch, change):
    keys = benchmark.case_keys({"example": None})[:2]
    count = 0

    def run(command, **options):
        nonlocal count
        count += 1
        if change == "failure":
            raise benchmark.subprocess.CalledProcessError(1, command)
        key = command[command.index("--only-case") + 1]
        if change == "wrong_case":
            key = "unexpected"
        output = Path(command[command.index("--json-output") + 1])
        output.write_text(
            json.dumps(
                {
                    "isolation": "changed"
                    if change == "metadata" and count == 2
                    else benchmark.ISOLATION,
                    "microseconds": {f"{key}/django": 10, f"{key}/rusty": 5},
                    "paired_ratios": {key: 0.5},
                }
            )
        )

    monkeypatch.setattr(benchmark.subprocess, "run", run)
    with pytest.raises((ValueError, benchmark.subprocess.CalledProcessError)):
        benchmark.isolated_report(keys, 20, 5, process_repeats=1)


def test_old_shared_process_baselines_are_rejected(tmp_path):
    report = {
        "python": "3.13",
        "django": "5.2",
        "fixture_sha256": "fixtures",
        "number": 20,
        "repeat": 5,
        "measurement": benchmark.MEASUREMENT,
        "minimum_duration": benchmark.MINIMUM_DURATION,
        "allocator_mmap_threshold": None,
        "isolation": benchmark.ISOLATION,
        "process_repeats": 3,
        "microseconds": {"case/django": 10, "case/rusty": 5},
        "paired_ratios": {"case": 0.5},
    }
    base = {key: value for key, value in report.items() if key != "isolation"}
    path = tmp_path / "base.json"
    path.write_text(json.dumps(base))
    args = argparse.Namespace(json_output=None, baseline=path, max_regression=0.15)
    with pytest.raises(SystemExit, match="Incompatible baseline: isolation"):
        benchmark.finish_report(report, args)


def test_isolation_preserves_regression_gate(tmp_path):
    report = {
        "python": "3.13",
        "django": "5.2",
        "fixture_sha256": "fixtures",
        "number": 20,
        "repeat": 5,
        "measurement": benchmark.MEASUREMENT,
        "minimum_duration": benchmark.MINIMUM_DURATION,
        "allocator_mmap_threshold": None,
        "isolation": benchmark.ISOLATION,
        "process_repeats": 3,
        "microseconds": {"case/django": 10, "case/rusty": 6},
        "paired_ratios": {"case": 0.6},
    }
    base = {**report, "paired_ratios": {"case": 0.5}}
    path = tmp_path / "base.json"
    path.write_text(json.dumps(base))
    args = argparse.Namespace(json_output=None, baseline=path, max_regression=0.15)
    with pytest.raises(SystemExit, match="1.20x"):
        benchmark.finish_report(report, args)


def test_real_children_preserve_fixture_identity_across_loop_and_text_cases():
    keys = [
        "debug=False/cached=False/exact_8/loaded",
        "debug=True/cached=True/shallow/from_string",
        "debug=False/cached=True/with_scopes_1000/loaded",
        "debug=True/cached=False/block_scopes_3/from_string",
    ]
    report = benchmark.isolated_report(keys, number=1, repeat=1, process_repeats=1)
    assert report["isolation"] == benchmark.ISOLATION
    assert set(report["paired_ratios"]) == set(keys)
    assert all(value > 0 for value in report["microseconds"].values())


def test_independent_process_median_rejects_an_isolated_control_outlier(monkeypatch):
    key = "debug=False/cached=False/example/loaded"
    ratios = iter([1.2, 1.8, 1.81])
    calls = []

    def run(command, **options):
        ratio = next(ratios)
        calls.append(command)
        output = Path(command[command.index("--json-output") + 1])
        output.write_text(
            json.dumps(
                {
                    "isolation": benchmark.ISOLATION,
                    "process_repeats": 1,
                    "microseconds": {f"{key}/django": 75 / ratio, f"{key}/rusty": 75},
                    "paired_ratios": {key: ratio},
                }
            )
        )

    monkeypatch.setattr(benchmark.subprocess, "run", run)
    report = benchmark.isolated_report([key], 20, 7)
    assert len(calls) == 3
    assert report["paired_ratios"][key] == 1.8
    assert report["microseconds"][f"{key}/django"] == 75 / 1.8
    assert report["process_repeats"] == 3
