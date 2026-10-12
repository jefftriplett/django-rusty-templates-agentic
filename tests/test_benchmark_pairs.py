import pytest

from scripts.benchmark_pairs import compare_pairs, measure_pairs


def test_pairs_calibrate_each_round_and_shuffle_order(monkeypatch):
    samples = {"rusty": iter([1, 8, 9]), "django": iter([1, 2, 30])}
    order = []

    def rusty():
        pass

    def django():
        pass

    def timing(operation, number):
        arm = operation.__name__
        order.append(arm)
        return next(samples[arm])

    monkeypatch.setattr("scripts.benchmark_pairs.timeit.timeit", timing)
    results, ratios = measure_pairs(
        {"case/rusty/render": (rusty, 1, 1), "case/django/render": (django, 1, 1)},
        repeat=3,
        minimum_duration=0,
    )
    # Ratios of separate medians would incorrectly report 4x. Pairing each
    # round retains the median 1x ratio despite the simulated timing drift.
    assert ratios == {"case/render": 1}
    assert results["case/rusty/render"] == 8e6
    assert set(order[::2]) == {"rusty", "django"}
    assert all(set(order[i : i + 2]) == {"rusty", "django"} for i in range(0, 6, 2))


def test_missing_control_is_rejected():
    with pytest.raises(ValueError, match="Missing paired engine"):
        measure_pairs({"case/rusty/render": (lambda: None, 1, 1)}, repeat=1)


def test_gate_still_rejects_real_regression():
    base = {"measurement": "paired-shuffled-v2", "paired_ratios": {"case": 0.5}}
    report = {"measurement": "paired-shuffled-v2", "paired_ratios": {"case": 0.55}}
    compare_pairs(base, report, 0.15)
    report["paired_ratios"]["case"] = 0.6
    with pytest.raises(SystemExit, match="1.20x"):
        compare_pairs(base, report, 0.15)


@pytest.mark.parametrize(
    "base",
    [
        {"paired_ratios": {"case": 1}},
        {"measurement": "paired-shuffled-v2", "paired_ratios": {"other": 1}},
    ],
)
def test_incompatible_baselines_are_rejected(base):
    report = {"measurement": "paired-shuffled-v2", "paired_ratios": {"case": 1}}
    with pytest.raises(SystemExit, match="Incompatible baseline"):
        compare_pairs(base, report, 0.15)


def test_short_samples_are_calibrated_without_changing_units(monkeypatch):
    counts = []

    def rusty():
        pass

    def django():
        pass

    def timing(operation, number):
        counts.append(number)
        return number * (2e-6 if operation is rusty else 1e-6)

    monkeypatch.setattr("scripts.benchmark_pairs.timeit.timeit", timing)
    results, ratios = measure_pairs(
        {"case/rusty/render": (rusty, 10, 1), "case/django/render": (django, 10, 1)},
        repeat=3,
    )
    assert results == {"case/rusty/render": 2, "case/django/render": 1}
    assert ratios == {"case/render": 2}
    assert min(counts[6:]) >= 5000
