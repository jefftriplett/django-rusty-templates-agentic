import pytest
from django.template import Context, Engine as DjangoEngine, TemplateDoesNotExist
from django.template.base import VariableNode

from django_rusty_templates import Engine


@pytest.fixture(params=[Engine, DjangoEngine])
def engine_class(request):
    return request.param


def test_string_origin_and_nodes(engine_class):
    engine = engine_class()
    template = engine.from_string("Hello {% if value %}{{ value }}{% endif %}")
    assert template.origin.name == "<unknown source>"
    assert template.origin.template_name is None
    assert template.origin.loader is None
    assert len(template.nodelist.get_nodes_by_type(VariableNode)) == 1
    assert template.render(Context({"value": "world"})) == "Hello world"
    engine.debug = True
    assert template.engine.debug


def test_loaded_origin_and_contents(engine_class):
    engine = engine_class(
        loaders=[("django.template.loaders.locmem.Loader", {"test.html": "Hi"})]
    )
    template = engine.get_template("test.html")
    loader = engine.template_loaders[0]
    assert template.origin.name == "test.html"
    assert template.origin.template_name == "test.html"
    assert template.origin.loader == loader
    assert template.origin.loader_name == "django.template.loaders.locmem.Loader"
    assert str(template.origin) == "test.html"
    assert loader.get_contents(template.origin) == "Hi"
    assert loader.get_template("test.html").render(Context()) == "Hi"
    missing = next(iter(loader.get_template_sources("missing.html")))
    with pytest.raises(TemplateDoesNotExist):
        loader.get_contents(missing)


def test_dirs_and_reset(engine_class, tmp_path):
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    (first / "test.html").write_text("first")
    (second / "test.html").write_text("second")
    engine = engine_class(dirs=[first])
    loader = engine.template_loaders[0]
    assert list(loader.get_dirs()) == [first]
    assert engine.get_template("test.html").render(Context()) == "first"
    assert "test.html" in loader.get_template_cache
    engine.dirs = [second]
    assert list(loader.get_dirs()) == [second]
    assert engine.get_template("test.html").render(Context()) == "first"
    loader.reset()
    assert loader.get_template_cache == {}
    template = engine.get_template("test.html")
    assert template.render(Context()) == "second"
    assert template.origin.loader == loader.loaders[0]
    assert template.origin.loader.get_contents(template.origin) == "second"
    sources = list(loader.get_template_sources("test.html"))
    assert sources[0] == template.origin
    assert not list(loader.get_template_sources("../outside.html"))


@pytest.mark.parametrize("debug", [False, True])
def test_cached_missing(engine_class, debug):
    engine = engine_class(debug=debug)
    loader = engine.template_loaders[0]
    with pytest.raises(TemplateDoesNotExist):
        engine.get_template("missing.html")
    cached = loader.get_template_cache["missing.html"]
    if debug:
        assert isinstance(cached, TemplateDoesNotExist)
        assert cached.__traceback__ is None
    else:
        assert cached is TemplateDoesNotExist
    assert loader.cache_key("missing.html") == "missing.html"
    loader.reset()
    assert loader.get_template_cache == {}


def test_cache_entries_survive_later_loads(engine_class, tmp_path):
    (tmp_path / "first.html").write_text("first")
    (tmp_path / "second.html").write_text("second")
    engine = engine_class(dirs=[tmp_path])
    loader = engine.template_loaders[0]
    engine.get_template("first.html")
    cache = loader.get_template_cache
    first = cache["first.html"]
    engine.get_template("second.html")
    with pytest.raises(TemplateDoesNotExist):
        engine.get_template("missing.html")
    assert loader.get_template_cache is cache
    assert cache["first.html"] is first
    assert set(cache) == {"first.html", "second.html", "missing.html"}


def test_explicit_dirs_remain_fixed(engine_class, tmp_path):
    engine = engine_class(
        loaders=[("django.template.loaders.filesystem.Loader", [tmp_path])]
    )
    engine.dirs = []
    assert list(engine.template_loaders[0].get_dirs()) == [tmp_path]
    assert callable(engine.template_context_processors[0])
