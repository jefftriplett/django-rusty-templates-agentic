from pathlib import Path

import pytest

from scripts.mutants import Worktree, apply_mutant, shard


def mutant(start, end, replacement):
    return {
        "span": {
            "start": {"line": start[0], "column": start[1]},
            "end": {"line": end[0], "column": end[1]},
        },
        "replacement": replacement,
    }


def test_apply_mutant_replaces_a_span_within_a_line():
    source = "fn f() -> bool {\n    a == b\n}\n"
    assert apply_mutant(source, mutant((2, 7), (2, 9), "!=")) == (
        "fn f() -> bool {\n    a != b\n}\n"
    )


def test_apply_mutant_replaces_a_span_across_lines():
    source = "fn f() -> u8 {\n    let x = 1;\n    x\n}\n"
    assert (
        apply_mutant(source, mutant((2, 5), (3, 6), "0"))
        == "fn f() -> u8 {\n    0\n}\n"
    )


def test_apply_mutant_counts_characters_not_bytes():
    source = 'let s = "é"; a && b\n'
    assert (
        apply_mutant(source, mutant((1, 16), (1, 18), "||")) == 'let s = "é"; a || b\n'
    )


def test_shards_split_round_robin():
    mutants = list(range(10))
    assert shard(mutants, "1/3") == [0, 3, 6, 9]
    assert shard(mutants, "3/3") == [2, 5, 8]
    for spec in ("0/3", "4/3", "x"):
        with pytest.raises(SystemExit):
            shard(mutants, spec)


def test_resume_ignores_results_from_other_commits(tmp_path):
    from scripts.mutants import load_done

    path = tmp_path / "results.jsonl"
    path.write_text(
        '{"name": "a", "outcome": "caught", "head": "new"}\n'
        '{"name": "b", "outcome": "survived", "head": "old"}\n'
    )
    assert set(load_done(path, "new")) == {"a"}
    assert load_done(tmp_path / "missing.jsonl", "new") == {}


def test_worktree_paths_are_absolute(tmp_path, monkeypatch):
    # CI passes `--output target/mutants`, and every command runs from
    # inside the worktree.
    monkeypatch.chdir(tmp_path)
    worktree = Worktree(Path("target/mutants/worktree"))
    assert worktree.path == tmp_path.resolve() / "target/mutants/worktree"
    for name in ("VIRTUAL_ENV", "PYO3_PYTHON"):
        assert Path(worktree.env[name]).is_absolute()
    assert Path(worktree.env["PATH"].split(":")[0]).is_absolute()
