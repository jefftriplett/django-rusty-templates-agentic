"""Mutation testing for the Rust engine, checked by the Python test suite.

cargo-mutants only runs `cargo test`, but most of our checks live in pytest.
This script asks cargo-mutants for the mutants (`--list --json`), then for
each one: applies it in a separate git worktree, rebuilds with
`maturin develop`, and runs pytest (then the Rust tests, if pytest passes).

A mutant that survives is a change to the engine that no test notices.

    uv run scripts/mutants.py -f src/render/tags.rs          # one file
    uv run scripts/mutants.py -f 'src/render/*.rs' --shard 1/4
    uv run scripts/mutants.py --in-diff changes.diff          # changed code only
    uv run scripts/mutants.py -f src/filters.rs --list        # just list

Results go to target/mutants/ (results.jsonl, survivors.md, survivors.diff).
Re-running at the same commit skips mutants already tested, so a run can be
stopped and resumed. The worktree is built from HEAD, so commit first.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

PROJECT_DIR = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT = PROJECT_DIR / "target" / "mutants"
PYTEST = ["pytest", "-x", "-q", "--no-header", "-p", "no:cacheprovider"]


def log(message):
    print(f"==> {message}", flush=True)


def list_mutants(args, cwd):
    # The whole workspace; .cargo/mutants.toml excludes the benchmark crate.
    command = ["cargo", "mutants", "--list", "--json", "--workspace"]
    for glob in args.file:
        command += ["--file", glob]
    for glob in args.exclude:
        command += ["--exclude", glob]
    for regex in args.re:
        command += ["--re", regex]
    for regex in args.exclude_re:
        command += ["--exclude-re", regex]
    if args.in_diff:
        command += ["--in-diff", str(args.in_diff.resolve())]
    output = subprocess.run(
        command, cwd=cwd, check=True, capture_output=True, text=True
    ).stdout
    # cargo-mutants prints nothing for an empty --in-diff.
    mutants = json.loads(output) if output.strip() else []
    if args.shard:
        mutants = shard(mutants, args.shard)
    return mutants[: args.limit] if args.limit else mutants


def shard(mutants, spec):
    """Every n-th mutant for shard `k/n`, so each shard gets a mix of files."""
    try:
        index, count = map(int, spec.split("/"))
    except ValueError:
        index = count = 0
    if not 1 <= index <= count:
        raise SystemExit("--shard must look like 1/4")
    return mutants[index - 1 :: count]


def apply_mutant(source, mutant):
    """Replace the mutant's span (1-based lines and character columns)."""
    lines = source.splitlines(keepends=True)
    start, end = mutant["span"]["start"], mutant["span"]["end"]

    def offset(position):
        before = sum(len(line) for line in lines[: position["line"] - 1])
        return before + position["column"] - 1

    return source[: offset(start)] + mutant["replacement"] + source[offset(end) :]


def run(command, cwd, env, timeout=None):
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout
        )
    except subprocess.TimeoutExpired:
        return "timeout", time.monotonic() - started, ""
    status = "ok" if completed.returncode == 0 else "failed"
    return status, time.monotonic() - started, completed.stdout + completed.stderr


class Worktree:
    def __init__(self, path):
        # Commands run from inside the worktree, so a relative path (CI
        # passes `--output target/mutants`) would point nowhere.
        path = Path(path).resolve()
        self.path = path
        venv = path / ".venv"
        bin_dir = venv / ("Scripts" if os.name == "nt" else "bin")
        site = next((venv / "lib").glob("python*/site-packages"), None)
        self.env = {
            **os.environ,
            "VIRTUAL_ENV": str(venv),
            "PATH": os.pathsep.join([str(bin_dir), os.environ["PATH"]]),
            "PYO3_PYTHON": str(bin_dir / "python"),
            "RUST_TEST_THREADS": "1",
        }
        if site:
            self.env["PYTHONPATH"] = os.pathsep.join([str(path), str(site)])
        self.env.pop("PYTHONHOME", None)

    @classmethod
    def create(cls, path):
        if not (path / ".git").exists():
            log(f"Creating worktree at {path}")
            path.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(
                ["git", "worktree", "add", "-f", "--detach", str(path), "HEAD"],
                cwd=PROJECT_DIR,
                check=True,
            )
        else:
            subprocess.run(["git", "checkout", "--", "."], cwd=path, check=True)
        head = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=PROJECT_DIR, text=True
        ).strip()
        subprocess.run(
            ["git", "checkout", "-q", "--detach", head], cwd=path, check=True
        )
        log("Syncing the worktree's virtualenv")
        subprocess.run(["uv", "sync", "-q"], cwd=path, check=True)
        worktree = cls(path)
        subprocess.run(
            ["uv", "run", "-m", "django", "compilemessages"],
            cwd=path,
            env=worktree.env,
            check=True,
            capture_output=True,
        )
        return worktree

    def build(self):
        return run(["maturin", "develop", "-q"], self.path, self.env)

    def pytest(self, timeout=None):
        return run(PYTEST, self.path, self.env, timeout)

    def cargo_test(self, timeout=None):
        command = ["cargo", "test", "--workspace", "-q", "--", "--test-threads=1"]
        return run(command, self.path, self.env, timeout)


def load_done(results_path, head):
    """Results already recorded for this commit; others are out of date."""
    if not results_path.exists():
        return {}
    done = {}
    for line in results_path.read_text().splitlines():
        if line.strip():
            result = json.loads(line)
            if result.get("head") == head:
                done[result["name"]] = result
    return done


def write_survivors(output, done, mutants):
    by_name = {mutant["name"]: mutant for mutant in mutants}
    survivors = sorted(name for name, r in done.items() if r["outcome"] == "survived")
    lines = [f"# Surviving mutants ({len(survivors)})", ""]
    for name in survivors:
        lines.append(f"- `{name}`")
    path = output / "survivors.md"
    path.write_text("\n".join(lines) + "\n")
    diffs = [by_name[name].get("diff", "") for name in survivors if name in by_name]
    (output / "survivors.diff").write_text("\n".join(filter(None, diffs)))
    return survivors


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("-f", "--file", action="append", default=[])
    parser.add_argument("-e", "--exclude", action="append", default=[])
    parser.add_argument("-F", "--re", action="append", default=[])
    parser.add_argument("-E", "--exclude-re", action="append", default=[])
    parser.add_argument("-D", "--in-diff", type=Path)
    parser.add_argument("--shard", help="Run every k-th mutant, as k/n")
    parser.add_argument("--limit", type=int, help="Stop after this many mutants")
    parser.add_argument("--list", action="store_true", help="List and exit")
    parser.add_argument("-o", "--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--no-cargo-test",
        action="store_true",
        help="Count a mutant as survived when pytest passes, without cargo test",
    )
    args = parser.parse_args()

    if not shutil.which("cargo-mutants"):
        raise SystemExit("cargo-mutants is not installed: cargo install cargo-mutants")
    if args.list:
        for mutant in list_mutants(args, PROJECT_DIR):
            print(mutant["name"])
        return 0

    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    results_path = args.output / "results.jsonl"
    worktree = Worktree.create(args.output / "worktree")
    head = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=worktree.path, text=True
    ).strip()
    done = load_done(results_path, head)
    # List inside the worktree so spans match the code being mutated.
    mutants = list_mutants(args, worktree.path)
    log(f"{len(mutants)} mutants")
    if not mutants:
        return 0

    log("Baseline: building and testing the unmutated tree")
    status, _, output = worktree.build()
    if status != "ok":
        print(output[-4000:])
        raise SystemExit("Baseline build failed")
    status, baseline, output = worktree.pytest()
    if status != "ok":
        print(output[-4000:])
        raise SystemExit("Baseline pytest failed")
    timeout = max(60, baseline * 3)
    log(f"Baseline pytest took {baseline:.0f}s; mutant timeout {timeout:.0f}s")

    with results_path.open("a") as results:
        for number, mutant in enumerate(mutants, 1):
            name = mutant["name"]
            if name in done:
                continue
            path = worktree.path / mutant["file"]
            original = path.read_text()
            path.write_text(apply_mutant(original, mutant))
            started = time.monotonic()
            try:
                status, _, _ = worktree.build()
                if status != "ok":
                    outcome = "unviable"
                else:
                    status, _, _ = worktree.pytest(timeout)
                    if status == "failed":
                        outcome = "caught"
                    elif status == "timeout":
                        outcome = "timeout"
                    elif args.no_cargo_test:
                        outcome = "survived"
                    else:
                        status, _, _ = worktree.cargo_test(timeout * 2)
                        outcome = (
                            "survived" if status == "ok" else "caught by cargo test"
                        )
            finally:
                path.write_text(original)
            result = {
                "name": name,
                "outcome": outcome,
                "seconds": round(time.monotonic() - started, 1),
                "head": head,
            }
            done[name] = result
            results.write(json.dumps(result) + "\n")
            results.flush()
            print(f"[{number}/{len(mutants)}] {outcome}: {name}", flush=True)

    # Leave the worktree's install matching HEAD.
    worktree.build()
    survivors = write_survivors(args.output, done, mutants)
    counts = {}
    for name in (mutant["name"] for mutant in mutants):
        outcome = done[name]["outcome"]
        counts[outcome] = counts.get(outcome, 0) + 1
    log(", ".join(f"{count} {outcome}" for outcome, count in sorted(counts.items())))
    log(f"Survivors: {args.output / 'survivors.md'}")
    return 1 if survivors else 0


if __name__ == "__main__":
    sys.exit(main())
