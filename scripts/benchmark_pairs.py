"""Measure matching Rusty/Django operations in shuffled, paired rounds."""

import random
import statistics
import timeit


MINIMUM_DURATION = 0.01
MEASUREMENT = "paired-shuffled-v2"


def measure_pair(
    operations, repeat, order_generator, minimum_duration=MINIMUM_DURATION
):
    """Calibrate short batches, then measure per-operation paired ratios."""
    numbers = {}
    for arm, (operation, number, _) in operations.items():
        if minimum_duration:
            elapsed = min(timeit.timeit(operation, number=number) for _ in range(3))
            number = max(number, int(number * minimum_duration / elapsed) + 1)
        numbers[arm] = number
    samples = {arm: [] for arm in operations}
    paired = []
    for _ in range(repeat):
        order = list(operations)
        order_generator.shuffle(order)
        for arm in order:
            operation, _, scale = operations[arm]
            number = numbers[arm]
            samples[arm].append(
                timeit.timeit(operation, number=number) / number * scale * 1e6
            )
        paired.append(samples["rusty"][-1] / samples["django"][-1])
    return (
        {arm: statistics.median(times) for arm, times in samples.items()},
        statistics.median(paired),
    )


def measure_pairs(operations, repeat, minimum_duration=MINIMUM_DURATION):
    """Operations map full case keys to (callable, minimum number, unit scale)."""
    groups = {}
    for key, operation in operations.items():
        arm = "rusty" if "/rusty/" in key else "django"
        pair = key.replace(f"/{arm}/", "/")
        groups.setdefault(pair, {})[arm] = (key, operation)
    results = {}
    ratios = {}
    order_generator = random.Random(0)
    for pair, arms in groups.items():
        if set(arms) != {"rusty", "django"}:
            raise ValueError(f"Missing paired engine: {pair}")
        medians, ratios[pair] = measure_pair(
            {arm: operation for arm, (_, operation) in arms.items()},
            repeat,
            order_generator,
            minimum_duration,
        )
        for arm, median in medians.items():
            results[arms[arm][0]] = median
    return results, ratios


def compare_pairs(base, report, max_regression):
    """Reject incompatible measurements and retain the normalized threshold."""
    if base.get("measurement") != report["measurement"]:
        raise SystemExit("Incompatible baseline: measurement")
    if base.get("minimum_duration") != report.get("minimum_duration"):
        raise SystemExit("Incompatible baseline: minimum duration")
    if base["paired_ratios"].keys() != report["paired_ratios"].keys():
        raise SystemExit("Incompatible baseline: paired cases")
    failures = []
    for key, value in report["paired_ratios"].items():
        ratio = value / base["paired_ratios"][key]
        if ratio > 1 + max_regression:
            failures.append(f"{key}: {ratio:.2f}x baseline relative to Django")
    if failures:
        raise SystemExit("Performance regression:\n" + "\n".join(failures))
