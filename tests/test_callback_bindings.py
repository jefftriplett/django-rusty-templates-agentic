"""Captured callable synchronization preserves Python object bindings."""

import pytest
from django.template import Context

from django_rusty_templates import Engine


class IdentityOnly:
    def __eq__(self, other):
        raise AssertionError("Binding synchronization must compare identity")


@pytest.mark.parametrize("debug", [False, True])
def test_callback_preserves_object_identity_without_equality(debug):
    original = IdentityOnly()
    replacement = IdentityOnly()
    context = Context({"items": [1, 2], "value": original})
    seen = []

    def callback():
        seen.append(context["value"])
        context["value"] = replacement
        return ""

    context["callback"] = callback
    Engine(debug=debug).from_string(
        "{% for i in items %}{{ callback }}{% endfor %}"
    ).render(context)
    assert seen[0] is original
    assert seen[1] is replacement
    assert context["value"] is original


@pytest.mark.parametrize("debug", [False, True])
def test_callback_parent_mutations_deletions_and_retained_flatten(debug):
    class RetainedDict(dict):
        def items(self):
            raise AssertionError("Dictionary overrides must not be called")

        def __contains__(self, key):
            raise AssertionError("Dictionary overrides must not be called")

    class RetainingContext(Context):
        def flatten(self):
            result = RetainedDict(super().flatten())
            self.flattened.append(result)
            return result

    context = RetainingContext({"items": [1, 2], "value": "parent"})
    context.flattened = []
    retained = []
    original_depth = len(context.dicts)

    def callback():
        retained.append(context.dicts[-1])
        del context.dicts[-1]["value"]
        context.dicts[1]["value"] = f"parent-{context['i']}"
        context.push({"added": f"added-{context['i']}"})
        return ""

    context["callback"] = callback
    source = "{% for i in items %}{{ callback }}{{ value }}/{{ added }};{% endfor %}"
    assert Engine(debug=debug).from_string(source).render(context) == (
        "parent-1/added-1;parent-2/added-2;"
    )
    assert len(context.dicts) == original_depth
    assert "added" not in context
    assert len({id(item) for item in retained}) == 2
    assert all(dict.__contains__(item, "items") for item in context.flattened)
    assert context.flattened[-1]["added"] == "added-2"


@pytest.mark.parametrize("debug", [False, True])
def test_callback_exception_retains_original_and_restores_stack(debug):
    context = Context({"items": [1], "value": "parent"})
    original = RuntimeError("callback failure")
    added = object()
    depth = len(context.dicts)

    def callback():
        context.push({"added": added})
        context["value"] = "changed"
        raise original

    context["callback"] = callback
    with pytest.raises(RuntimeError) as caught:
        Engine(debug=debug).from_string(
            "{% for i in items %}{{ callback }}{% endfor %}"
        ).render(context)
    assert caught.value is original
    assert len(context.dicts) == depth
    assert context["value"] == "changed"
    assert context["added"] is added


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("failure", [False, True])
def test_non_string_binding_fallback_preserves_original_error(debug, failure):
    context = Context({"items": [1]})
    original = RuntimeError("original callback error")

    def callback():
        context.dicts[-1][object()] = "unsupported binding key"
        if failure:
            raise original
        return ""

    context["callback"] = callback
    depth = len(context.dicts)
    with pytest.raises(RuntimeError if failure else TypeError) as caught:
        Engine(debug=debug).from_string(
            "{% for i in items %}{{ callback }}{% endfor %}"
        ).render(context)
    if failure:
        assert caught.value is original
    assert len(context.dicts) == depth
    assert context.template is None


@pytest.mark.parametrize("debug", [False, True])
def test_reentrant_callback_render_restores_native_bindings(debug):
    context = Context({"items": [1, 2], "value": "outside"})
    engine = Engine(debug=debug)
    inner = engine.from_string("{{ i }}/{{ value }}")
    retained = []

    def callback():
        retained.append(context.dicts[-1])
        context["value"] = f"inner-{context['i']}"
        return inner.render(context)

    context["callback"] = callback
    output = engine.from_string(
        "{% for i in items %}{{ callback }};{% endfor %}/{{ value }}"
    ).render(context)
    assert output == "1/inner-1;2/inner-2;/outside"
    assert context["value"] == "outside"
    assert "i" not in context
    assert [snapshot["i"] for snapshot in retained] == [1, 2]


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("cached", [False, True])
@pytest.mark.parametrize("loaded", [False, True])
def test_large_flat_context_uses_live_bindings_without_flatten(
    monkeypatch, tmp_path, debug, cached, loaded
):
    source = "{{ callback }}{{ callback }}{{ value }}"
    (tmp_path / "flat.html").write_text(source)
    loader = "django.template.loaders.filesystem.Loader"
    loaders = (
        [("django.template.loaders.cached.Loader", [loader])] if cached else [loader]
    )
    engine = Engine(dirs=[str(tmp_path)], loaders=loaders, debug=debug)
    template = (
        engine.get_template("flat.html") if loaded else engine.from_string(source)
    )
    original = IdentityOnly()
    replacement = IdentityOnly()
    context = Context(
        {"value": "<&>", "object": original, **{f"key{i}": i for i in range(1000)}}
    )
    retained = []
    seen = []
    depth = len(context.render_context.dicts)

    def callback():
        seen.append(context["object"])
        retained.append(context.dicts[-1])
        context["object"] = replacement
        context.autoescape = False
        assert context.template is not None
        assert context.render_context.template is not None
        return ""

    context["callback"] = callback

    def unexpected(context):
        raise AssertionError("Flat live rendering must not flatten/import context")

    monkeypatch.setattr(Context, "flatten", unexpected)
    assert template.render(context) == "<&>"
    assert seen[0] is original and seen[1] is replacement
    assert all(item is context.dicts[-1] for item in retained)
    assert context["object"] is replacement
    assert context.autoescape is False
    assert context.template is None
    assert len(context.render_context.dicts) == depth


@pytest.mark.parametrize("debug", [False, True])
def test_large_flat_context_subclass_keeps_compatibility_flatten(debug):
    class CustomContext(Context):
        def flatten(self):
            self.calls += 1
            return super().flatten()

    context = CustomContext({"value": "ok", **{f"key{i}": i for i in range(1000)}})
    context.calls = 0
    assert Engine(debug=debug).from_string("{{ value }}").render(context) == "ok"
    assert context.calls == 1


@pytest.mark.parametrize("debug", [False, True])
def test_large_flat_callback_failure_restores_binding_and_keeps_original(debug):
    context = Context({f"key{i}": i for i in range(1000)})
    depth = len(context.render_context.dicts)
    original = RuntimeError("flat callback failure")

    def callback():
        context["added"] = original
        context.autoescape = False
        raise original

    context["callback"] = callback
    with pytest.raises(RuntimeError) as caught:
        Engine(debug=debug).from_string("{{ callback }}").render(context)
    assert caught.value is original
    assert context["added"] is original
    assert context.autoescape is False
    assert context.template is None
    assert context.render_context.template is None
    assert len(context.render_context.dicts) == depth
