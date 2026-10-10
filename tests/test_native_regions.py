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
