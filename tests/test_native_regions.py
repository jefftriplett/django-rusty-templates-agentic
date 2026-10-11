import pytest
from django.template import Context, Engine as DjangoEngine
from django.template.base import VariableNode

from django_rusty_templates import Engine
from django_rusty_templates.interop import NativeRegionNode, compile_template


@pytest.mark.parametrize("debug", [False, True])
def test_regions_preserve_variable_inspection_and_debug_nodes(debug):
    source = "<p>{{ first }} / {{ second }}</p>"
    template = compile_template(source, Engine(debug=debug))
    assert len(template.nodelist.get_nodes_by_type(VariableNode)) == 2
    assert len(template.nodelist.get_nodes_by_type(NativeRegionNode)) == (
        0 if debug else 1
    )
    assert template.render(Context({"first": "<b>", "second": "Café"})) == (
        "<p>&lt;b&gt; / Café</p>"
    )


@pytest.mark.parametrize(
    "source",
    [
        "{{ first }}{# removed #}{ {{ second }} / {{ third }}",
        "{{ first }}{% comment %}{{ ignored }}{% endcomment %}{{ second }} / {{ third }}",
        "{{ first }}{% verbatim %}{{ ignored }}{% endverbatim %}{{ second }} / {{ third }}",
        "{% autoescape off %}{{ first }} / {{ second }}{% endautoescape %}{{ first }} / {{ second }}",
        "{% for first in rows %}{{ first }} / {{ second }}{% empty %}empty{% endfor %}",
    ],
)
def test_regions_preserve_comments_verbatim_escape_and_loop_boundaries(source):
    values = {"first": "<b>", "second": "<i>", "third": "&", "rows": ["<u>", "&"]}
    expected = DjangoEngine().from_string(source).render(Context(values))
    assert compile_template(source, Engine()).render(Context(values)) == expected


@pytest.mark.parametrize("debug", [False, True])
def test_regions_read_context_changes_between_variables(debug):
    source = "{{ mutate }} / {{ value }} / {{ value|default:fallback }}"

    def context():
        result = Context({"value": "old", "fallback": "fallback"})

        def mutate():
            result.push(value="<new>")
            result.autoescape = False
            return "<mutated>"

        result["mutate"] = mutate
        return result

    expected = DjangoEngine(debug=debug).from_string(source).render(context())
    assert expected == "<mutated> / <new> / <new>"
    template = compile_template(source, Engine(debug=debug))
    assert template.render(context()) == expected


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("rows", [[], [("<a>", [1, 2]), ("&", [])]])
def test_structural_regions_preserve_nested_scopes_and_empty_branches(debug, rows):
    source = (
        "{{ name }}:{% for row in rows reversed %}"
        "{{ forloop.counter }}={{ row.0 }}:"
        "{% if row.1 %}{% for number in row.1 %}{{ number }};{% endfor %}"
        "{% else %}empty{% endif %}/{% empty %}no rows{% endfor %}/{{ name }}"
    )
    values = {"name": "outside", "rows": rows}
    expected = DjangoEngine(debug=debug).from_string(source).render(Context(values))
    template = compile_template(source, Engine(debug=debug))
    assert template.render(Context(values)) == expected
    if not debug:
        assert len(template.nodelist) == 1
        region = template.nodelist[0]
        assert isinstance(region, NativeRegionNode)
        assert region._native is not None
        assert all(
            node._native is None
            for node in region.nodelist.get_nodes_by_type(VariableNode)
        )


def test_structural_region_callbacks_see_and_modify_live_loop_scope():
    source = "{% for row in rows %}{{ mutate }}:{{ row }}:{{ value }};{% endfor %}{{ row }}:{{ value }}"

    def context():
        result = Context({"rows": [1, 2], "row": "outside", "value": "before"})

        def mutate():
            counter = result["forloop"]["counter"]
            result["row"] = f"changed-{result['row']}"
            result["value"] = counter
            return counter

        result["mutate"] = mutate
        return result

    expected = DjangoEngine().from_string(source).render(context())
    assert expected == "1:changed-1:1;2:changed-2:2;outside:before"
    assert compile_template(source, Engine()).render(context()) == expected


def test_structural_region_restores_context_after_callback_error():
    source = "{% for row in rows %}{{ fail }}:{{ row }}{% endfor %}"
    context = Context({"rows": [1], "row": "outside"})

    def fail():
        assert context["row"] == 1
        raise RuntimeError("callback failed")

    context["fail"] = fail
    depth = len(context.dicts)
    with pytest.raises(RuntimeError, match="callback failed"):
        compile_template(source, Engine()).render(context)
    assert len(context.dicts) == depth
    assert context["row"] == "outside"
    assert "forloop" not in context


def test_regions_keep_django_parent_loops_across_unsupported_nodes():
    source = (
        "{% for row in rows %}{% autoescape off %}{{ row }}{% endautoescape %}"
        "{% for value in row %}{{ forloop.parentloop.counter }}:{{ value }};{% endfor %}"
        "{% endfor %}"
    )
    values = {"rows": [[1, 2], [3]]}
    expected = DjangoEngine().from_string(source).render(Context(values))
    assert compile_template(source, Engine()).render(Context(values)) == expected


def test_structural_region_preserves_live_counter_changes_and_dictionary_identity():
    source = "{% for row in rows %}{{ mutate }}:{{ forloop.counter }};{% endfor %}"

    def context():
        result = Context({"rows": [1, 2]})
        previous = []

        def mutate():
            current = result["forloop"]
            if previous:
                assert current is previous[0]
                del current["counter"]
            else:
                previous.append(current)
                current["counter"] = 10
            return "mutated"

        result["mutate"] = mutate
        return result

    expected = DjangoEngine().from_string(source).render(context())
    assert expected == "mutated:10;mutated:;"
    assert compile_template(source, Engine()).render(context()) == expected


def test_structural_regions_keep_unpacking_scopes_in_django():
    source = "{% for first, second in rows %}{{ mutate }}:{{ value }};{% endfor %}{{ value }}"

    def context():
        result = Context({"rows": [(1, 2), (3, 4)], "value": "outside"})

        def mutate():
            before = result["value"]
            result["value"] = result["first"]
            return before

        result["mutate"] = mutate
        return result

    expected = DjangoEngine().from_string(source).render(context())
    assert compile_template(source, Engine()).render(context()) == expected


@pytest.mark.parametrize("autoescape", [True, False])
def test_region_string_fast_path_preserves_subclass_hooks(autoescape):
    class CustomString(str):
        def __str__(self):
            return "<str>"

        def __html__(self):
            return "<html>"

    source = "{% for value in values %}{{ value }};{% endfor %}"
    values = {"values": ["<>&\"'é", CustomString("ignored")]}
    expected = (
        DjangoEngine()
        .from_string(source)
        .render(Context(values, autoescape=autoescape))
    )
    assert (
        compile_template(source, Engine()).render(
            Context(values, autoescape=autoescape)
        )
        == expected
    )


def test_inherited_block_bodies_are_grouped():
    source = (
        '{% extends "base.html" %}{% block body %}'
        "<p>{{ first }} / {{ second }}</p>{% endblock %}"
    )
    engine = Engine(
        loaders=[
            (
                "django.template.loaders.locmem.Loader",
                {
                    "base.html": "[{% block body %}base{% endblock %}]",
                },
            )
        ]
    )
    template = compile_template(source, engine)
    regions = template.nodelist.get_nodes_by_type(NativeRegionNode)
    assert len(regions) == 1
    assert regions[0].source == "<p>{{ first }} / {{ second }}</p>"
    assert template.render(Context({"first": "<", "second": "&"})) == (
        "[<p>&lt; / &amp;</p>]"
    )


def test_equivalent_origin_binding_does_not_walk_nodes(monkeypatch):
    from django.template.base import Node, Origin

    template = compile_template("{% block body %}{{ value }}{% endblock %}", Engine())
    loader = object()
    origin = Origin("page.html", template_name="page.html", loader=loader)
    template.bind_origin(origin)
    nodes = template.nodelist.get_nodes_by_type(Node)
    assert all(node.origin is origin for node in nodes)

    def unexpected_walk(*args):
        raise AssertionError("Repeated origin binding walked the template tree")

    with monkeypatch.context() as patch:
        patch.setattr(template.nodelist, "get_nodes_by_type", unexpected_walk)
        template.bind_origin(origin)
        template.bind_origin(
            Origin("page.html", template_name="page.html", loader=loader)
        )

    replacement = Origin("page.html", template_name="alias.html", loader=loader)
    template.bind_origin(replacement)
    assert template.origin is replacement
    assert all(node.origin is replacement for node in nodes)
