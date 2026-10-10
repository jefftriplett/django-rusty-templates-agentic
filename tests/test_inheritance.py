import pytest
from django.template import Context, TemplateSyntaxError

from django_rusty_templates import Engine
from django_rusty_templates.inheritance import NativeVariableNode


def engine_with_templates(templates):
    return Engine(loaders=[("django.template.loaders.locmem.Loader", templates)])


def test_nested_inheritance_renders_native_variables_and_super():
    engine = engine_with_templates(
        {
            "base.html": "[{% block content %}base {{ value }}{% endblock %}]",
            "middle.html": '{% extends "base.html" %}{% block content %}middle {{ block.super }}{% endblock %}',
            "child.html": '{% extends "middle.html" %}{% block content %}child {{ block.super }}{% endblock %}',
        }
    )
    template = engine.get_template("child.html")
    assert template.render({"value": "<"}) == "[child middle base &lt;]"
    assert template.nodelist.get_nodes_by_type(NativeVariableNode)
    assert template.render({"value": "next"}) == "[child middle base next]"


def test_extends_native_template_object():
    engine = Engine()
    parent = engine.from_string("{% block body %}parent{% endblock %}")
    child = engine.from_string(
        "{% extends parent %}{% block body %}child {{ block.super }}{% endblock %}"
    )
    assert child.render(Context({"parent": parent})) == "child parent"


def test_cached_recursive_inheritance_has_live_origin_cache(tmp_path):
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    (first / "same.html").write_text(
        '{% extends "same.html" %}{% block body %}child {{ block.super }}{% endblock %}'
    )
    (second / "same.html").write_text("{% block body %}parent{% endblock %}")
    (first / "other.html").write_text('{% extends "same.html" %}')
    engine = Engine(dirs=[first, second])
    template = engine.get_template("same.html")
    assert template.render() == "child parent"
    cache = engine.template_loaders[0].get_template_cache
    assert len(cache) == 2
    assert engine.get_template("other.html").render() == "child parent"
    assert len(cache) == 3
    engine.template_loaders[0].reset()
    assert cache == {}


def test_duplicate_blocks_are_rejected():
    with pytest.raises(TemplateSyntaxError, match="appears more than once"):
        Engine().from_string(
            "{% block body %}{% endblock %}{% block body %}{% endblock %}"
        )
