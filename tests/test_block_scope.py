"""Block and `{% with %}` bodies take the parent's block table and caches
instead of copying them, and hand them back afterwards (#367)."""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library

from django_rusty_templates import Engine

TEMPLATES = {
    "base.html": "<{% block a %}A{% endblock %}|{% block b %}B{% block inner %}I"
    "{% endblock %}{% endblock %}|{% block c %}C{% endblock %}>",
    "middle.html": "{% extends 'base.html' %}{% block a %}mA[{{ block.super }}]"
    "{% endblock %}{% block inner %}mI[{{ block.super }}]{% endblock %}",
    "child.html": "{% extends 'middle.html' %}{% block a %}cA[{{ block.super }}]"
    "{% endblock %}{% block b %}cB[{{ block.super }}]{% endblock %}"
    "{% block c %}{% for x in items %}{% cycle 'p' 'q' %}{% ifchanged x %}{{ x }}"
    "{% endifchanged %}{% endfor %}{% endblock %}",
    "cycles.html": "{% extends 'base.html' %}{% block a %}{% for x in items %}"
    "{% cycle 'r' 's' 't' as c silent %}{% endfor %}{{ c }}{% endblock %}"
    "{% block c %}{% for x in items %}{% include 'row.html' %}{% endfor %}"
    "{% endblock %}",
    "row.html": "({{ x }})",
    "caught.html": "{% extends 'middle.html' %}{% load scope %}{% block a %}"
    "{% attempt 'bad.html' %}[{{ block.super }}]{% endblock %}"
    "{% block c %}{% attempt 'badwith.html' %}{{ block.super }}{% endblock %}",
    # Without `extends`: a tag rendering a child template inside an inherited
    # block shares the outer block table, so it uses a block name of its own (#391).
    "bad.html": "{% block zz %}{{ boom.boom }}{% endblock %}",
    "badwith.html": "{% with a=1 %}{% block z %}{{ boom.boom }}{% endblock %}{% endwith %}",
    "withs.html": "{% extends 'base.html' %}{% block a %}{% with a=1 %}{% with b=2 %}"
    "{{ a }}{{ b }}[{{ block.super }}]{% endwith %}{% endwith %}{% endblock %}"
    "{% block c %}{% with c=3 %}{% include 'row.html' with x=c %}{% endwith %}"
    "{% endblock %}",
}


class Boom:
    @property
    def boom(self):
        raise ValueError("boom")


@pytest.fixture
def engines(monkeypatch):
    module = ModuleType("block_scope_tags")
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
            libraries={"scope": module.__name__},
            loaders=[("django.template.loaders.locmem.Loader", TEMPLATES)],
        )
        return current["engine"]

    return build


@pytest.mark.parametrize(
    "name", ["child.html", "cycles.html", "caught.html", "withs.html"]
)
def test_blocks_match_django(engines, name):
    context = {"items": [1, 1, 2, 3, 3], "boom": Boom()}

    def render(engine_class):
        template = engines(engine_class).get_template(name)
        # Twice, so state handed back by one render can't leak into the next.
        return [template.render(Context(dict(context))) for _ in range(2)]

    expected = render(DjangoEngine)
    assert render(Engine) == expected
    assert expected[0] == expected[1]


@pytest.mark.parametrize("count", [1, 30, 120])
def test_many_blocks_with_super(count):
    base = "".join(f"{{% block b{i} %}}{i}{{% endblock %}}" for i in range(count))
    child = "{% extends 'base.html' %}" + "".join(
        f"{{% block b{i} %}}<{{{{ block.super }}}}>{{% endblock %}}"
        for i in range(count)
    )
    options = {
        "loaders": [
            (
                "django.template.loaders.locmem.Loader",
                {"base.html": base, "child.html": child},
            )
        ]
    }
    expected = DjangoEngine(**options).get_template("child.html").render(Context())
    assert Engine(**options).get_template("child.html").render({}) == expected
