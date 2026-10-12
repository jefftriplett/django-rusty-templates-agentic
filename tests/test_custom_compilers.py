import sys
from types import ModuleType

import pytest
from django.template import Context, Library, Node

from django_rusty_templates import Engine
from django_rusty_templates.interop import NativeVariableNode


@pytest.fixture
def custom_engine(monkeypatch):
    module = ModuleType("rusty_custom_compiler_tests")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)
    return module.register, Engine(libraries={"custom": module.__name__}, debug=True)


class CaptureNode(Node):
    def __init__(self, expression, nodelist):
        self.expression = expression
        self.nodelist = nodelist

    def render(self, context):
        context["greeting"] = self.expression.resolve(context)
        count = context.render_context.get("captures", 0) + 1
        context.render_context["captures"] = count
        return f"{count}:{self.nodelist.render(context)}"


def test_custom_block_compiler_shares_context_and_renders_native_children(
    custom_engine,
):
    library, engine = custom_engine

    @library.tag("capture")
    def compile_capture(parser, token):
        expression = parser.compile_filter(token.split_contents()[1])
        nodes = parser.parse(("endcapture",))
        parser.delete_first_token()
        parser.extra_data["capture"] = "compiled"
        return CaptureNode(expression, nodes)

    template = engine.from_string(
        "{% load custom %}{% capture value %}{{ greeting }}{% endcapture %}|"
        "{% capture other %}{{ greeting }}{% endcapture %}"
    )
    context = Context({"value": "<", "other": "second"})
    assert template.render(context) == "1:&lt;|2:second"
    assert context["greeting"] == "second"
    assert template.render(context) == "1:&lt;|2:second"
    assert template.extra_data == {"capture": "compiled"}
    assert len(template.nodelist.get_nodes_by_type(NativeVariableNode)) == 2


class ConstantNode(Node):
    def __init__(self, value):
        self.value = value

    def render(self, context):
        return self.value


def test_custom_compiler_with_arbitrary_closure(custom_engine):
    library, engine = custom_engine

    def make_compiler(value):
        def compile_constant(parser, token):
            return ConstantNode(value)

        return compile_constant

    library.tag("constant", make_compiler("closure"))
    assert engine.from_string("{% load custom %}{% constant %}").render() == "closure"


def test_callable_object_compiler(custom_engine):
    library, engine = custom_engine

    class Compiler:
        def __call__(self, parser, token):
            return ConstantNode("callable")

    library.tag("constant", Compiler())
    assert engine.from_string("{% load custom %}{% constant %}").render() == "callable"


def test_custom_compile_error_preserves_original_exception(custom_engine):
    library, engine = custom_engine
    error = RuntimeError("compiler failed")

    @library.tag("broken")
    def compile_broken(parser, token):
        raise error

    with pytest.raises(RuntimeError) as caught:
        engine.from_string("{% load custom %}{% broken %}")
    assert caught.value is error
    assert error.template_debug["during"] == "{% broken %}"


def test_explain_template_reports_custom_fallback_location(custom_engine):
    library, engine = custom_engine

    @library.tag("constant")
    def compile_constant(parser, token):
        return ConstantNode("custom")

    source = "{% load custom %}\nCafé\n{% constant %}"
    report = engine.explain_template(source)
    assert report == {
        "mode": "hybrid",
        "reason": "not yet implemented: Fully custom tag",
        "tag": "{% constant %}",
        "span": (len("{% load custom %}\nCafé\n".encode()), len("{% constant %}")),
        "line": 3,
    }
    assert engine.from_string(source).render() == "\nCafé\ncustom"


def test_explain_template_reports_native_and_rejects_invalid_syntax():
    engine = Engine()
    assert engine.explain_template("{% for i in values %}{{ i }}{% endfor %}") == {
        "mode": "native",
        "reason": None,
        "tag": None,
        "span": None,
        "line": None,
    }
    from django.template import TemplateSyntaxError

    with pytest.raises(TemplateSyntaxError):
        engine.explain_template("{% for i in values %}")
