"""Opaque compiler boundaries preserve Django semantics without parsing outer nodes."""

import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node
from django.template.base import Parser

from django_rusty_templates import Engine
from django_rusty_templates import interop


class CaptureNode(Node):
    child_nodelists = ("nodelist",)

    def __init__(self, expression, nodelist):
        self.expression = expression
        self.nodelist = nodelist

    def render(self, context):
        context["captured"] = self.expression.resolve(context)
        context.render_context[self] = context.render_context.get(self, 0) + 1
        return f"{context.render_context[self]}:{self.nodelist.render(context)}"


@pytest.fixture
def libraries(monkeypatch):
    module = ModuleType("rusty_isolated_compiler_tests")
    module.register = Library()
    calls = []

    @module.register.tag("capture")
    def capture(parser, token):
        calls.append(token)
        token._parser_origin = parser.origin
        parser.capture_count = getattr(parser, "capture_count", 0) + 1
        parser.extra_data["captures"] = parser.capture_count
        expression = parser.compile_filter(token.split_contents()[1])
        nodes = parser.parse(("endcapture",))
        parser.delete_first_token()
        return CaptureNode(expression, nodes)

    capture.rusty_isolated_compiler = True
    monkeypatch.setitem(sys.modules, module.__name__, module)
    return module.register, {"custom": module.__name__}, calls


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("cached", [False, True])
def test_native_outer_parse_preserves_scopes_and_node_identity(
    tmp_path, monkeypatch, libraries, debug, cached
):
    _, libraries, calls = libraries
    source = (
        "{% load custom %}Café\n"
        "{% if enabled %}{% for value in values %}"
        "{% capture value|upper %}{{ captured }}-{{ forloop.counter }}"
        "{% endcapture %}|{% endfor %}{% endif %}{{ captured }}"
    )
    (tmp_path / "custom.html").write_text(source)
    loaders = ["django.template.loaders.filesystem.Loader"]
    if cached:
        loaders = [("django.template.loaders.cached.Loader", loaders)]
    options = dict(
        dirs=[str(tmp_path)], libraries=libraries, debug=debug, loaders=loaders
    )
    reference = DjangoEngine(**options).get_template("custom.html")
    data = {"enabled": True, "values": ["<", "second"]}
    expected_context = Context(data)
    expected = reference.render(expected_context)
    calls.clear()
    parse_spans = []
    original = Parser.parse

    def record(parser, parse_until=None):
        parse_spans.append((parse_until, parser.tokens[-1].lineno))
        return original(parser, parse_until)

    monkeypatch.setattr(Parser, "parse", record)
    monkeypatch.setattr(
        interop,
        "compile_template",
        lambda *args: pytest.fail("whole-template compatibility parser entered"),
    )
    template = Engine(**options).get_template("custom.html")
    assert len(calls) == 1
    assert parse_spans == [(("endcapture",), 2)]
    assert calls[0].lineno == 2
    assert calls[0]._parser_origin.name == str(tmp_path / "custom.html")
    assert calls[0]._parser_origin.template_name == "custom.html"
    assert (
        calls[0].position
        == (
            source.index("{% capture"),
            source.index("{% capture") + len("{% capture value|upper %}"),
        )
        if debug
        else calls[0].position is None
    )
    assert template.extra_data == {"captures": 1}
    context = Context(data)
    assert template.render(context) == expected == "Café\n1:&lt;-1|2:SECOND-2|"
    assert context.flatten() == expected_context.flatten()
    assert template.render(Context(data)) == expected
    assert len(calls) == 1
    assert len(parse_spans) == 1


def test_persistent_parser_custom_state_and_nested_nodes(libraries, monkeypatch):
    _, modules, calls = libraries
    source = (
        "{% load custom %}{% capture a %}{{ captured }}{% capture b %}"
        "{{ captured }}{% endcapture %}{{ captured }}{% endcapture %}"
        "{% capture c %}{{ captured }}{% endcapture %}"
    )
    monkeypatch.setattr(
        interop, "compile_template", lambda *args: pytest.fail("fallback")
    )
    template = Engine(libraries=modules).from_string(source)
    assert template.extra_data == {"captures": 3}
    assert len(calls) == 3
    assert template.render(Context({"a": "a", "b": "b", "c": "c"})) == "1:a1:bb1:c"


def test_compiler_exception_is_not_replayed(libraries):
    library, modules, _ = libraries
    error = RuntimeError("compiler failed")
    calls = []

    @library.tag("broken")
    def broken(parser, token):
        calls.append(token)
        raise error

    broken.rusty_isolated_compiler = True
    with pytest.raises(RuntimeError) as caught:
        Engine(libraries=modules, debug=True).from_string(
            "Café\n{% load custom %}{% broken %}"
        )
    assert caught.value is error
    assert len(calls) == 1
    assert error.template_debug["during"] == "{% broken %}"


def test_explain_does_not_invoke_opted_in_compiler(libraries):
    _, modules, calls = libraries
    source = "{% load custom %}{% capture value %}{{ value }}{% endcapture %}"
    assert Engine(libraries=modules).explain_template(source)["mode"] == "hybrid"
    assert calls == []


@pytest.mark.parametrize(
    "suffix", ["{% block x %}x{% endblock %}", "{% cycle 'a' 'b' %}"]
)
def test_shared_native_parser_registries_keep_compatibility(
    libraries, monkeypatch, suffix
):
    _, modules, calls = libraries
    entries = []
    original = interop.compile_template

    def record(*args):
        entries.append(args[0])
        return original(*args)

    monkeypatch.setattr(interop, "compile_template", record)
    source = (
        "{% load custom %}{% capture value %}{{ captured }}{% endcapture %}" + suffix
    )
    template = Engine(libraries=modules).from_string(source)
    assert len(entries) == 1
    assert len(calls) == 1
    assert template.render(Context({"value": "ok"})).startswith("1:ok")


def test_load_in_opaque_body_supplies_filters_to_native_suffix(libraries, monkeypatch):
    _, modules, _ = libraries
    extra = ModuleType("rusty_isolated_extra_tests")
    extra.register = Library()
    extra.register.filter("suffix", lambda value: value + "!")
    monkeypatch.setitem(sys.modules, extra.__name__, extra)
    modules["extra"] = extra.__name__
    source = (
        "{% load custom %}{% capture value %}{% load extra %}"
        "{{ captured|suffix }}{% endcapture %}|{{ value|suffix }}"
    )
    monkeypatch.setattr(
        interop, "compile_template", lambda *args: pytest.fail("fallback")
    )
    template = Engine(libraries=modules).from_string(source)
    assert template.render(Context({"value": "ok"})) == "1:ok!|ok!"


def test_unannotated_late_compiler_is_checked_before_opted_in_compiler(libraries):
    library, modules, calls = libraries

    @library.tag("legacy")
    def legacy(parser, token):
        from django.template.base import TextNode

        return TextNode("")

    source = (
        "{% load custom %}{% capture value %}{{ captured }}{% endcapture %}{% legacy %}"
    )
    template = Engine(libraries=modules).from_string(source)
    assert len(calls) == 1
    assert template.render(Context({"value": "ok"})) == "1:ok"


def test_native_syntax_error_after_compiler_does_not_replay(libraries):
    from django.template import TemplateSyntaxError

    _, modules, calls = libraries
    source = "{% load custom %}{% capture value %}ok{% endcapture %}{{ value|not_registered }}"
    with pytest.raises(TemplateSyntaxError):
        Engine(libraries=modules).from_string(source)
    assert len(calls) == 1


def test_isolated_callable_supports_next_token_and_pushback(libraries, monkeypatch):
    library, modules, _ = libraries
    identities = []

    class Echo(Node):
        def __init__(self, nodes):
            self.nodelist = nodes

        def render(self, context):
            return self.nodelist.render(context)

    class Compiler:
        rusty_isolated_compiler = True

        def __call__(self, parser, token):
            first = parser.next_token()
            parser.prepend_token(first)
            identities.append(first)
            nodes = parser.parse(("endecho",))
            parser.delete_first_token()
            assert nodes[0].token is first
            return Echo(nodes)

    library.tag("echo", Compiler())
    source = "{% load custom %}{% echo %}body{{ value }}{% endecho %}{{ value }}"
    monkeypatch.setattr(
        interop, "compile_template", lambda *args: pytest.fail("fallback")
    )
    template = Engine(libraries=modules).from_string(source)
    assert template.render(Context({"value": "ok"})) == "bodyokok"
    assert len(identities) == 1


def test_replacing_loaded_custom_compiler_updates_native_suffix(libraries, monkeypatch):
    from django.template.base import TextNode

    library, modules, _ = libraries

    @library.tag("echo")
    def echo(parser, token):
        return TextNode("before")

    echo.rusty_isolated_compiler = True

    @library.tag("replace")
    def replace(parser, token):
        def replacement(parser, token):
            return TextNode("after")

        replacement.rusty_isolated_compiler = True
        parser.tags["echo"] = replacement
        return TextNode("")

    replace.rusty_isolated_compiler = True
    monkeypatch.setattr(
        interop, "compile_template", lambda *args: pytest.fail("fallback")
    )
    source = "{% load custom %}{% echo %}{% replace %}{% echo %}"
    assert Engine(libraries=modules).from_string(source).render() == "beforeafter"


@pytest.mark.parametrize("debug", [False, True])
def test_late_native_error_matches_django_without_replaying_compiler(libraries, debug):
    from django.template import TemplateSyntaxError

    _, modules, calls = libraries
    source = "Café\n{% load custom %}{% capture value %}ok{% endcapture %}{{ value|not_registered }}"
    with pytest.raises(TemplateSyntaxError) as expected:
        DjangoEngine(libraries=modules, debug=debug).from_string(source)
    calls.clear()
    with pytest.raises(TemplateSyntaxError) as caught:
        Engine(libraries=modules, debug=debug).from_string(source)
    assert str(caught.value) == str(expected.value)
    assert len(calls) == 1
    if debug:
        assert caught.value.template_debug["during"] == "{{ value|not_registered }}"


def test_render_exception_restores_input_context_state(libraries):
    library, modules, _ = libraries
    error = RuntimeError("render failed")

    class Broken(Node):
        def render(self, context):
            raise error

    @library.tag("broken")
    def broken(parser, token):
        return Broken()

    broken.rusty_isolated_compiler = True
    context = Context({"value": "ok"})
    before = len(context.render_context.dicts)
    template = Engine(libraries=modules, debug=True).from_string(
        "{% load custom %}{% broken %}"
    )
    with pytest.raises(RuntimeError) as caught:
        template.render(context)
    assert caught.value is error
    assert context.template is None
    assert len(context.render_context.dicts) == before
    assert error.template_debug["during"] == "{% broken %}"


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("body", ["{{ value }}", "{% if value %}{{ value }}"])
def test_unclosed_opaque_block_matches_django(libraries, debug, body):
    from django.template import TemplateSyntaxError

    _, modules, calls = libraries
    source = "Café\n{% load custom %}{% capture value %}" + body
    with pytest.raises(TemplateSyntaxError) as expected:
        DjangoEngine(libraries=modules, debug=debug).from_string(source)
    calls.clear()
    with pytest.raises(TemplateSyntaxError) as caught:
        Engine(libraries=modules, debug=debug).from_string(source)
    assert str(caught.value) == str(expected.value)
    assert len(calls) == 1
    if debug:
        assert (
            caught.value.template_debug["during"]
            == expected.value.template_debug["during"]
        )


def test_skip_past_unclosed_tag_matches_django(libraries):
    from django.template import TemplateSyntaxError
    from django.template.base import TextNode

    library, modules, _ = libraries
    calls = []

    @library.tag("skip")
    def skip(parser, token):
        calls.append(token)
        parser.skip_past("endskip")
        return TextNode("")

    skip.rusty_isolated_compiler = True
    source = "{% load custom %}{% skip %}ignored"
    with pytest.raises(TemplateSyntaxError) as expected:
        DjangoEngine(libraries=modules, debug=True).from_string(source)
    calls.clear()
    with pytest.raises(TemplateSyntaxError) as caught:
        Engine(libraries=modules, debug=True).from_string(source)
    assert str(caught.value) == str(expected.value)
    assert len(calls) == 1
    assert caught.value.template_debug["during"] == "{% skip %}"


def test_callback_replaces_native_assignment_before_writeback(libraries):
    _, modules, _ = libraries
    source = "{% load custom %}{% now 'Y' as captured %}{% capture value %}{{ captured }}{% endcapture %}{{ captured }}"
    expected_context = Context({"value": "after"})
    expected = (
        DjangoEngine(libraries=modules).from_string(source).render(expected_context)
    )
    context = Context({"value": "after"})
    actual = Engine(libraries=modules).from_string(source).render(context)
    assert actual == expected == "1:afterafter"
    assert context["captured"] == expected_context["captured"] == "after"


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("parent_callback", [False, True])
def test_native_parent_loop_includes_opaque_child_with_scoped_assignments(
    tmp_path, libraries, debug, parent_callback
):
    _, modules, _ = libraries
    parent = "{% for value in values %}{% include 'child.html' %}{{ captured }}{{ value }}|{% endfor %}"
    if parent_callback:
        parent = "{% load custom %}{% capture 'root' %}{% endcapture %}" + parent
    child = "{% load custom %}{% capture value %}{{ captured }}-{{ forloop.counter }}{% endcapture %}"
    (tmp_path / "parent.html").write_text(parent)
    (tmp_path / "child.html").write_text(child)
    options = dict(dirs=[str(tmp_path)], libraries=modules, debug=debug)
    data = {"values": ["a", "b"]}
    expected = DjangoEngine(**options).get_template("parent.html").render(Context(data))
    actual = Engine(**options).get_template("parent.html").render(Context(data))
    assert actual == expected
    assert actual == (
        "1:1:a-1roota|1:b-2rootb|" if parent_callback else "1:a-1a|1:b-2b|"
    )


def test_request_context_processors_run_once_and_keep_caller_precedence(
    libraries, monkeypatch
):
    from django.http import HttpRequest
    from django.template import RequestContext

    _, modules, _ = libraries
    module = sys.modules[modules["custom"]]
    calls = []

    def processor(request):
        calls.append(request)
        return {"value": "processor", "from_processor": "ready"}

    module.processor = processor
    options = dict(
        libraries=modules, context_processors=[module.__name__ + ".processor"]
    )
    source = "{% load custom %}{% capture from_processor %}{{ captured }}:{{ value }}{% endcapture %}"
    request = HttpRequest()
    reference_context = RequestContext(request)
    reference_context.push({"value": "caller"})
    expected = DjangoEngine(**options).from_string(source).render(reference_context)
    calls.clear()
    context = RequestContext(request)
    context.push({"value": "caller"})
    original = context.dicts[context._processors_index]
    actual = Engine(**options).from_string(source).render(context)
    assert actual == expected == "1:ready:caller"
    assert calls == [request]
    assert context.dicts[context._processors_index] is original
    assert context.template is None


def test_invalid_compiler_result_is_not_replayed(libraries):
    library, modules, _ = libraries
    calls = []

    @library.tag("invalid")
    def invalid(parser, token):
        calls.append(token)
        return None

    invalid.rusty_isolated_compiler = True
    source = "{% load custom %}{% invalid %}"
    with pytest.raises(AttributeError) as expected:
        DjangoEngine(libraries=modules).from_string(source)
    calls.clear()
    with pytest.raises(AttributeError) as caught:
        Engine(libraries=modules).from_string(source)
    assert str(caught.value) == str(expected.value)
    assert len(calls) == 1


def test_syntax_diagnostic_does_not_invoke_opted_in_compiler(libraries):
    _, modules, calls = libraries
    source = "{% load custom %}{% capture value %}body{% endcapture %}"
    Engine(libraries=modules)._syntax_diagnostic(source, None, None)
    assert calls == []


@pytest.mark.parametrize("debug", [False, True])
def test_opted_in_compiler_mixes_with_native_stock_tags(libraries, monkeypatch, debug):
    _, modules, calls = libraries
    modules.update(
        {name: "django.templatetags." + name for name in ["static", "cache", "i18n"]}
    )
    source = (
        "{% load custom static cache i18n %}{% static 'main.css' %}|"
        "{% cache 0 isolated-stock %}{% language 'en' %}"
        "{% capture value %}{{ captured }}{% endcapture %}"
        "{% endlanguage %}{% endcache %}|{{ captured }}"
    )
    options = dict(libraries=modules, debug=debug)
    expected = (
        DjangoEngine(**options).from_string(source).render(Context({"value": "ok"}))
    )
    calls.clear()
    spans = []
    original = Parser.parse

    def record(parser, parse_until=None):
        spans.append(parse_until)
        return original(parser, parse_until)

    monkeypatch.setattr(Parser, "parse", record)
    monkeypatch.setattr(
        interop, "compile_template", lambda *args: pytest.fail("fallback")
    )
    template = Engine(**options).from_string(source)
    assert template.render(Context({"value": "ok"})) == expected
    assert len(calls) == 1
    assert spans == [("endcapture",)]


def test_unannotated_compile_func_name_does_not_bypass_preflight(libraries):
    from django.template.base import TextNode

    library, modules, calls = libraries

    def compile_func(parser, token):
        return TextNode("legacy")

    library.tag("legacy", compile_func)
    source = (
        "{% load custom %}{% capture value %}{{ captured }}{% endcapture %}{% legacy %}"
    )
    template = Engine(libraries=modules).from_string(source)
    assert len(calls) == 1
    assert template.render(Context({"value": "ok"})) == "1:oklegacy"


def test_dynamic_unannotated_replacement_reports_contract_error_once(libraries):
    from django.template import TemplateSyntaxError
    from django.template.base import TextNode

    library, modules, _ = libraries
    calls = []

    @library.tag("replace")
    def replace(parser, token):
        calls.append(token)
        parser.tags["late"] = lambda parser, token: TextNode("late")
        return TextNode("")

    replace.rusty_isolated_compiler = True
    with pytest.raises(
        TemplateSyntaxError, match="registered a compiler requiring the full parser"
    ):
        Engine(libraries=modules).from_string(
            "{% load custom %}{% replace %}{% late %}"
        )
    assert len(calls) == 1


class ReadOnlyControlFlow(BaseException):
    def __setattr__(self, name, value):
        raise RuntimeError("exception attributes are readonly")


@pytest.mark.parametrize(
    "exception_type",
    [KeyboardInterrupt, SystemExit, BaseException, ReadOnlyControlFlow],
)
@pytest.mark.parametrize("debug", [False, True])
def test_compiler_control_flow_exception_is_not_replayed_or_annotated(
    libraries, exception_type, debug
):
    library, modules, _ = libraries
    error = exception_type("interrupted")
    calls = []

    @library.tag("interrupt")
    def interrupt(parser, token):
        calls.append(token)
        raise error

    interrupt.rusty_isolated_compiler = True
    source = "{% load custom %}{% interrupt %}"
    with pytest.raises(exception_type) as reference:
        DjangoEngine(libraries=modules, debug=debug).from_string(source)
    assert reference.value is error
    assert len(calls) == 1
    calls.clear()
    with pytest.raises(exception_type) as caught:
        Engine(libraries=modules, debug=debug).from_string(source)
    assert caught.value is error
    assert len(calls) == 1
    for attribute in [
        "token",
        "template_debug",
        "rusty_diagnostic",
        "_rusty_compiler_error",
    ]:
        assert not hasattr(error, attribute)


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("interface", ["string", "region"])
def test_readonly_exception_annotation_failure_does_not_replay_compiler(
    libraries, debug, interface
):
    library, modules, _ = libraries
    calls = []

    class ReadOnlyException(Exception):
        def __setattr__(self, name, value):
            raise AttributeError("exception attributes are readonly")

    @library.tag("frozen")
    def frozen(parser, token):
        calls.append(token)
        raise ReadOnlyException("original failure")

    frozen.rusty_isolated_compiler = True
    source = "{% load custom %}{% frozen %}"
    with pytest.raises(AttributeError) as expected:
        DjangoEngine(libraries=modules, debug=debug).from_string(source)
    assert len(calls) == 1
    calls.clear()
    with pytest.raises(AttributeError) as caught:
        engine = Engine(libraries=modules, debug=debug)
        if interface == "string":
            engine.from_string(source)
        else:
            engine._compile_interop_region(source, [])
    assert type(caught.value) is type(expected.value)
    assert str(caught.value) == str(expected.value)
    assert len(calls) == 1
