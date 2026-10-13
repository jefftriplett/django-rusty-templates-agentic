import gc
import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library
from django_rusty_templates import Engine


@pytest.mark.parametrize(
    "scope",
    ["{% with local='inner' %}x{% endwith %}", "{% block body %}x{% endblock %}"],
)
def test_scope_does_not_delay_replaced_assignment_destructor(monkeypatch, scope):
    outcomes = []
    for cls in (DjangoEngine, Engine):
        events = []
        module = ModuleType("scoped_lifetime_tags")
        module.register = Library()
        monkeypatch.setitem(sys.modules, module.__name__, module)

        class Value:
            def __del__(self):
                events.append("dropped")

        module.register.simple_tag(Value, name="make")
        module.register.simple_tag(lambda: "new", name="replace")
        module.register.simple_tag(lambda: ",".join(events), name="probe")
        source = (
            "{% load tags %}{% make as held %}"
            + scope
            + "{% replace as held %}|{% probe %}"
        )
        template = cls(libraries={"tags": module.__name__}).from_string(source)
        outcomes.append(template.render(Context()))
    assert outcomes == ["x|dropped", "x|dropped"]


@pytest.mark.parametrize(
    "scope",
    ["{% with local='inner' %}x{% endwith %}", "{% block body %}x{% endblock %}"],
)
def test_retained_native_context_keeps_snapshot_on_parent_mutation(monkeypatch, scope):
    retained = []
    module = ModuleType("scoped_snapshot_tags")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    @module.register.simple_tag(takes_context=True)
    def remember(context):
        retained.append(context)
        return ""

    module.register.simple_tag(lambda: "new", name="replace")
    engine = Engine(libraries={"tags": module.__name__})
    source = (
        "{% load tags %}" + scope + "{% remember %}{% replace as value %}{{ value }}"
    )
    assert engine.from_string(source).render(Context({"value": "original"})) == "xnew"
    assert retained[0]["value"] == "original"
    retained.clear()
    gc.collect()


@pytest.mark.parametrize(
    "scope",
    [
        "{% with value='inner' %}{{ value }}{% endwith %}",
        "{% block body %}{% with value='inner' %}{{ value }}{% endwith %}{% endblock %}",
    ],
)
def test_large_context_scopes_preserve_shadowing_and_root_values(scope):
    values = {f"key{index}": str(index) for index in range(1000)}
    values["value"] = "outer"
    source = scope + "|{{ value }}|{{ key999 }}"
    for cls in (DjangoEngine, Engine):
        assert cls().from_string(source).render(Context(values)) == "inner|outer|999"


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize(
    "source",
    [
        "{% for value in rows %}{% with local=value %}"
        "{% for value in rows %}{{ value }}{% endfor %}{{ local }}"
        "{% endwith %}{{ value }}{% endfor %}|{{ value }}",
        '{% cycle "a" "b" as item silent %}{% with local=1 %}'
        "{% cycle item %}{% resetcycle item %}{% cycle item %}"
        "{% endwith %}|{{ item }}|{% cycle item %}",
        "{% for value in rows %}{% with local=value %}"
        "{% ifchanged local %}{{ local }}{% else %}={% endifchanged %}"
        "{% endwith %}{% endfor %}|{{ value }}",
    ],
)
def test_shared_scope_loop_and_render_state_matches_django(debug, source):
    values = {f"key{i}": str(i) for i in range(256)}
    values.update(value="outer", rows=["a", "a", "b"])
    outcomes = []
    for cls in (DjangoEngine, Engine):
        template = cls(debug=debug).from_string(source)
        outcomes.append(template.render(Context(values)))
    assert outcomes[0] == outcomes[1]
