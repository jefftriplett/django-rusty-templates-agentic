import pytest
from django.template import Context, Engine as DjangoEngine
from django.template.base import Node, Parser, VariableNode

from django_rusty_templates import Engine


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("loaded", [False, True])
def test_flat_inspection_never_parses_and_preserves_metadata(
    monkeypatch, debug, loaded
):
    source = "你好\n{{ value|default:'fallback' }}{# hidden #}{{ other }}"
    loaders = [("django.template.loaders.locmem.Loader", {"page.html": source})]
    native = Engine(debug=debug, loaders=loaders)
    django = DjangoEngine(debug=debug, loaders=loaders)
    template = (
        native.get_template("page.html") if loaded else native.from_string(source)
    )
    reference = (
        django.get_template("page.html") if loaded else django.from_string(source)
    )

    def unexpected(*args, **kwargs):
        raise AssertionError("Flat native inspection invoked Parser.parse")

    monkeypatch.setattr(Parser, "parse", unexpected)
    nodes = template.nodelist
    assert template.nodelist is nodes
    assert template._is_native
    assert len(nodes.get_nodes_by_type(VariableNode)) == 2
    assert nodes.contains_nontext == reference.nodelist.contains_nontext
    for actual, expected in zip(
        nodes.get_nodes_by_type(Node),
        reference.nodelist.get_nodes_by_type(Node),
        strict=True,
    ):
        assert type(actual) is type(expected)
        assert actual.token.contents == expected.token.contents
        assert actual.token.lineno == expected.token.lineno
        assert actual.token.position == expected.token.position
        assert actual.origin.name == expected.origin.name
        assert actual.origin.template_name == expected.origin.template_name
    context = Context({"value": "<value>", "other": "last"})
    assert template.render(context) == reference.render(context)
    assert nodes.render(context) == reference.nodelist.render(context)


@pytest.mark.parametrize("debug", [False, True])
def test_complex_inspection_compatibility_parse_is_cached(monkeypatch, debug):
    source = "{% if value %}{{ value }}{% endif %}"
    reference = DjangoEngine(debug=debug).from_string(source)
    template = Engine(debug=debug).from_string(source)
    calls = []
    original = Parser.parse

    def tracked(parser, *args, **kwargs):
        calls.append(parser)
        return original(parser, *args, **kwargs)

    monkeypatch.setattr(Parser, "parse", tracked)
    assert template.render(Context({"value": "yes"})) == "yes"
    assert not calls
    nodes = template.nodelist
    assert calls
    count = len(calls)
    assert nodes.get_nodes_by_type(VariableNode)
    for actual, expected in zip(
        nodes.get_nodes_by_type(Node),
        reference.nodelist.get_nodes_by_type(Node),
        strict=True,
    ):
        assert type(actual) is type(expected)
        assert actual.token.position == expected.token.position
        assert actual.token.lineno == expected.token.lineno
        assert actual.origin.name == expected.origin.name
    assert template.nodelist is nodes
    assert template.render(Context({"value": "yes"})) == "yes"
    assert len(calls) == count
    assert template._is_native
