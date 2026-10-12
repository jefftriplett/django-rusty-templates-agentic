"""Measure matching Rusty/Django operations in shuffled, paired rounds."""

import random
import statistics
import timeit


def measure_pairs(operations, repeat):
    """Operations map full case keys to (callable, number, unit scale)."""
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
        samples = {arm: [] for arm in arms}
        paired = []
        for _ in range(repeat):
            order = list(arms)
            order_generator.shuffle(order)
            for arm in order:
                _, (operation, number, scale) = arms[arm]
                samples[arm].append(
                    timeit.timeit(operation, number=number) / number * scale * 1e6
                )
            paired.append(samples["rusty"][-1] / samples["django"][-1])
        for arm, times in samples.items():
            results[arms[arm][0]] = statistics.median(times)
        ratios[pair] = statistics.median(paired)
    return results, ratios


def compare_pairs(base, report, max_regression):
    """Reject incompatible measurements and retain the normalized threshold."""
    if base.get("measurement") != report["measurement"]:
        raise SystemExit("Incompatible baseline: measurement")
    if base["paired_ratios"].keys() != report["paired_ratios"].keys():
        raise SystemExit("Incompatible baseline: paired cases")
    failures = []
    for key, value in report["paired_ratios"].items():
        ratio = value / base["paired_ratios"][key]
        if ratio > 1 + max_regression:
            failures.append(f"{key}: {ratio:.2f}x baseline relative to Django")
    if failures:
        raise SystemExit("Performance regression:\n" + "\n".join(failures))
