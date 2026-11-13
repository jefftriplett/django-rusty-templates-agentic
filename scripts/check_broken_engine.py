"""Fail when tests still pass while the Rusty engine is broken on purpose.

Runs pytest with scripts/broken_engine_plugin.py in each mode and compares
the tests that survive with scripts/broken_engine_allowlist.txt:

- output: a Rusty render succeeded, returned junk, and the test passed, so
  the test never checks what Rusty rendered.
- raise: every Rusty compile and render raised, and the test passed anyway.
  Django reference variants (tests parametrized as Django, or that compile
  Django templates and aren't parametrized as Rusty) are expected to pass
  and aren't reported.

Allowlist lines are `<mode> <pattern>  # reason`, where the pattern is an
fnmatch pattern for pytest node ids. Patterns that match nothing fail too,
so the list shrinks as tests get fixed.

    uv run scripts/check_broken_engine.py               # both modes
    uv run scripts/check_broken_engine.py --mode output
    uv run scripts/check_broken_engine.py --mode raise -- tests/tags
"""

import argparse
import concurrent.futures
import fnmatch
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
PROJECT_DIR = SCRIPT_DIR.parent
ALLOWLIST = SCRIPT_DIR / "broken_engine_allowlist.txt"
MODES = ("output", "raise")
RUSTY_PARAM = re.compile(r"(^|[-\[])rusty(_nocache)?([-\]]|$)")
DJANGO_PARAM = re.compile(
    r"(^|[-\[])(django(_nocache)?|DjangoTemplates|DjangoEngine)([-\]]|$)"
)


def param_matches(pattern, nodeid):
    match = re.search(r"\[.*\]$", nodeid)
    return bool(match and pattern.search(match.group()))


def survivors(mode, results):
    found = set()
    for nodeid, result in results.items():
        if not result["passed"]:
            continue
        if mode == "output":
            if result["rendered"]:
                found.add(nodeid)
        elif param_matches(RUSTY_PARAM, nodeid):
            found.add(nodeid)
        # Django reference variants pass by design. Ids name most of them;
        # the rest compile a Django template (unless Django's loader cached it).
        elif not (param_matches(DJANGO_PARAM, nodeid) or result["django"]):
            found.add(nodeid)
    return found


def load_allowlist(path):
    entries = []
    for number, line in enumerate(path.read_text().splitlines(), 1):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        mode, _, pattern = line.partition(" ")
        if mode not in MODES or not pattern.strip():
            raise SystemExit(f"{path.name}:{number}: expected '<mode> <pattern>'")
        entries.append((mode, pattern.strip()))
    return entries


def run_pytest(mode, pytest_args):
    with tempfile.TemporaryDirectory() as directory:
        results_path = Path(directory) / "results.json"
        pythonpath = [str(SCRIPT_DIR)]
        if os.environ.get("PYTHONPATH"):
            pythonpath.append(os.environ["PYTHONPATH"])
        command = [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "--tb=no",
            "--no-header",
            "-p",
            "no:cacheprovider",
            "-p",
            "broken_engine_plugin",
            *pytest_args,
        ]
        completed = subprocess.run(
            command,
            cwd=PROJECT_DIR,
            env={
                **os.environ,
                "BROKEN_ENGINE": mode,
                "BROKEN_ENGINE_RESULTS": str(results_path),
                "PYTHONPATH": os.pathsep.join(pythonpath),
            },
            capture_output=True,
            text=True,
        )
        summary = completed.stdout.strip().splitlines()[-1:] or ["(no output)"]
        print(f"==> {mode}: {' '.join(command[2:])}\n    {summary[0]}", flush=True)
        if not results_path.exists():
            print(completed.stdout[-4000:], completed.stderr[-4000:], sep="\n")
            raise SystemExit(f"{mode}: pytest did not record results")
        return json.loads(results_path.read_text())


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--mode", choices=MODES, action="append")
    parser.add_argument("--allowlist", type=Path, default=ALLOWLIST)
    parser.add_argument(
        "--write-allowlist",
        action="store_true",
        help="Print allowlist lines for today's survivors instead of checking",
    )
    parser.add_argument("pytest_args", nargs="*")
    args = parser.parse_args()
    modes = args.mode or list(MODES)
    entries = [] if args.write_allowlist else load_allowlist(args.allowlist)
    # Stale patterns only mean something on a full run.
    full_run = not args.pytest_args
    failed = False
    # The modes are separate pytest processes, so they run side by side.
    with concurrent.futures.ThreadPoolExecutor() as pool:
        runs = [pool.submit(run_pytest, mode, args.pytest_args) for mode in modes]
    for mode, run in zip(modes, runs):
        found = survivors(mode, run.result())
        if args.write_allowlist:
            for nodeid in sorted(found):
                print(f"{mode} {nodeid}")
            continue
        patterns = [pattern for entry_mode, pattern in entries if entry_mode == mode]
        unexpected = sorted(
            nodeid
            for nodeid in found
            if not any(fnmatch.fnmatchcase(nodeid, p) for p in patterns)
        )
        stale = [
            pattern
            for pattern in patterns
            if full_run and not any(fnmatch.fnmatchcase(n, pattern) for n in found)
        ]
        print(f"    {len(found)} survivors, {len(unexpected)} not allowlisted")
        if unexpected:
            failed = True
            print(f"\nTests that pass with the Rusty engine broken ({mode} mode):")
            for nodeid in unexpected:
                print(f"  {nodeid}")
            print(
                "\nMake them assert on Rusty's output or errors. If a test "
                f"is right as is, add it to {args.allowlist.name} with a reason.\n"
            )
        if stale:
            failed = True
            print(f"\nAllowlist patterns that no longer match ({mode} mode):")
            for pattern in stale:
                print(f"  {pattern}")
            print()
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
