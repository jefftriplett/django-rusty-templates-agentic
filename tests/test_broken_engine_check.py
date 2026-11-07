import pytest

from scripts.check_broken_engine import load_allowlist, survivors


def result(passed=True, rendered=0, django=0):
    return {"passed": passed, "rendered": rendered, "django": django, "rusty": 1}


def test_output_survivors_are_passing_tests_that_rendered():
    results = {
        "a[rusty]": result(rendered=1),
        "b[rusty]": result(rendered=1, passed=False),
        "c[rusty]": result(rendered=0),
    }
    assert survivors("output", results) == {"a[rusty]"}


def test_raise_survivors_skip_django_reference_variants():
    results = {
        "t.py::named[django]": result(),
        "t.py::named[DjangoEngine-x]": result(),
        "t.py::compiles_django[Engine1]": result(django=1),
        "t.py::rusty_compiles_django[rusty]": result(django=1),
        "t.py::touches_nothing": result(),
        "t.py::failed[rusty]": result(passed=False),
    }
    assert survivors("raise", results) == {
        "t.py::rusty_compiles_django[rusty]",
        "t.py::touches_nothing",
    }


def test_rusty_nocache_is_a_rusty_variant():
    assert survivors("raise", {"t.py::x[rusty_nocache-1]": result(django=1)})


def test_allowlist_parsing(tmp_path):
    path = tmp_path / "allow.txt"
    path.write_text("# comment\n\noutput a::b  # reason\nraise c::*\n")
    assert load_allowlist(path) == [("output", "a::b"), ("raise", "c::*")]
    path.write_text("render a::b\n")
    with pytest.raises(SystemExit, match="allow.txt:1"):
        load_allowlist(path)
