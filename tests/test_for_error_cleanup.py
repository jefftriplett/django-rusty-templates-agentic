"""A `{% for %}` that fails leaves no loop state behind (#382).

A tag can render another template with its own context and catch the error.
Django's loop closes its scope on the way out, so the outer template carries
on without the failed loop's variables or `forloop`.
"""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library

from django_rusty_templates import Engine


class Boom:
    @property
    def boom(self):
        raise ValueError("boom")


TEMPLATES = {
    "items.html": "{% for x in items %}{{ x.boom }}{% endfor %}",
    "string.html": "{% for x in 'ab' %}{{ b.boom }}{% endfor %}",
    "second.html": "{% for x in 'ab' %}{% if forloop.last %}{{ b.boom }}{% endif %}"
    "{% endfor %}",
    "unpack.html": "{% for x, y in items %}{{ x }}{% endfor %}",
    "nested.html": "{% for x in 'ab' %}{% for y in items %}{{ y.boom }}{% endfor %}"
    "{% endfor %}",
    "iterating.html": "{% for x in failing %}{{ x }}{% endfor %}",
}
AFTER = "[{{ x }}][{{ y }}][{{ forloop.counter }}]"


def failing():
    yield 1
    raise ValueError("iteration failed")


@pytest.fixture
def engines(monkeypatch):
    module = ModuleType("for_error_cleanup_tags")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)
    current = {}

    @module.register.simple_tag(takes_context=True)
    def attempt(context, name):
        try:
            return current["engine"].get_template(name).render(context)
        except ValueError:
            return "caught"

    def build(engine_class):
        current["engine"] = engine_class(
            libraries={"cleanup": module.__name__},
            loaders=[("django.template.loaders.locmem.Loader", TEMPLATES)],
        )
        return current["engine"]

    return build


@pytest.mark.parametrize("name", sorted(TEMPLATES))
@pytest.mark.parametrize(
    "source",
    [
        "{{% load cleanup %}}{{% attempt '{name}' %}}" + AFTER,
        "{{% load cleanup %}}{{% for x in 'pq' %}}{{% attempt '{name}' %}}"
        + AFTER
        + "{{% endfor %}}"
        + AFTER,
        "{{% load cleanup %}}{{% for x in 'pq' %}}{{% for y in 'rs' %}}"
        "{{% attempt '{name}' %}}" + AFTER + "{{% endfor %}}{{% endfor %}}",
    ],
)
def test_failed_loop_leaves_no_state(engines, name, source):
    source = source.format(name=name) if "{name}" in source else source
    source = source.replace("{{%", "{%").replace("%}}", "%}")

    def render(engine_class):
        context = {"items": [Boom(), Boom()], "b": Boom(), "failing": failing()}
        return engines(engine_class).from_string(source).render(Context(context))

    expected = render(DjangoEngine)
    assert "caught" in expected
    assert render(Engine) == expected
