import pytest
from django.template import Engine as DjangoEngine

from django_rusty_templates import Engine


@pytest.mark.parametrize(
    "options",
    [
        {},
        {
            "dirs": ["templates"],
            "debug": True,
            "autoescape": False,
            "string_if_invalid": "missing",
            "file_charset": "utf-16",
            "loaders": ["django.template.loaders.filesystem.Loader"],
            "libraries": {"custom": "tests.templatetags.custom_tags"},
            "context_processors": ["django.template.context_processors.debug"],
        },
    ],
)
def test_engine_repr(options):
    assert repr(Engine(**options)) == repr(DjangoEngine(**options))


def test_repr_tracks_changed_options():
    rusty, django = Engine(), DjangoEngine()
    for engine in [rusty, django]:
        engine.dirs = ["changed"]
        engine.debug = True
    assert repr(rusty) == repr(django)


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
def test_template_source(engine_class):
    source = "Hello {{ value }}"
    assert engine_class().from_string(source).source == source


def test_dirs_mutation_affects_loading(tmp_path):
    (tmp_path / "test.html").write_text("Hello")
    for engine_class in (Engine, DjangoEngine):
        engine = engine_class()
        engine.dirs.append(tmp_path)
        assert engine.get_template("test.html").source == "Hello"
