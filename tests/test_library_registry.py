import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine
from tests.templatetags.more_filters import register


@pytest.mark.parametrize("engine_cls", [Engine, DjangoEngine])
def test_library_registry_and_builtins(engine_cls):
    engine = engine_cls(
        libraries={"alias": "tests.templatetags.more_filters"},
        builtins=["tests.templatetags.more_filters"],
    )
    assert engine.template_libraries["alias"] is register
    assert engine.template_builtins[-1] is register
    assert engine.builtins[-1] == "tests.templatetags.more_filters"
    assert (
        engine.from_string("{{ value|square }}").render(Context({"value": 4})) == "16"
    )
    assert (
        engine.from_string("{% load alias %}{{ value|square }}").render(
            Context({"value": 5})
        )
        == "25"
    )


@pytest.mark.parametrize("engine_cls", [Engine, DjangoEngine])
def test_builtin_filters_override_defaults(engine_cls):
    from django.template import Library
    import sys
    from types import ModuleType

    library = Library()
    library.filter("upper", lambda value: "custom:" + value)
    module = ModuleType("test_builtin_override")
    module.register = library
    sys.modules[module.__name__] = module
    try:
        engine = engine_cls(builtins=[module.__name__])
        assert (
            engine.from_string("{{ value|upper }}").render(Context({"value": "value"}))
            == "custom:value"
        )
    finally:
        del sys.modules[module.__name__]
