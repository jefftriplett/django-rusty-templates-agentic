"""Input that used to kill the process raises the exception Django raises.

A segfault or abort here ends the whole test run, so a pass means the guard
held.
"""

import sys
import threading

import django
import pytest
from django.template import Context, Engine as DjangoEngine, VariableDoesNotExist

from django_rusty_templates import Engine

TEMPLATES = {
    "self.html": 'x{% include "self.html" %}',
    "tree.html": (
        "{{ node.value }}{% for child in node.children %}{% if child %}"
        '{% with node=child %}{% include "tree.html" %}{% endwith %}'
        "{% endif %}{% endfor %}"
    ),
    "isolated.html": '{% include "isolated.html" only %}',
    "pair_a.html": '{% include "pair_b.html" %}',
    "pair_b.html": '{% include "pair_a.html" %}',
}
LOADERS = [
    (
        "django.template.loaders.cached.Loader",
        [("django.template.loaders.locmem.Loader", TEMPLATES)],
    )
]
ENGINES = [DjangoEngine, Engine]


def tree(depth):
    node = {"value": depth, "children": []}
    for value in reversed(range(depth)):
        node = {"value": value, "children": [node]}
    return node


@pytest.mark.parametrize("engine_class", ENGINES)
@pytest.mark.parametrize("name", ["self.html", "isolated.html", "pair_a.html"])
def test_endless_include_raises_recursion_error(engine_class, name):
    template = engine_class(loaders=LOADERS).get_template(name)
    with pytest.raises(RecursionError):
        template.render(Context({}))


@pytest.mark.parametrize("engine_class", ENGINES)
def test_recursive_include_renders_a_tree(engine_class):
    engine = engine_class(loaders=LOADERS)
    template = engine.get_template("tree.html")
    expected = "".join(str(value) for value in range(21))
    assert template.render(Context({"node": tree(20)})) == expected
    # An endless include doesn't leave the engine unable to render.
    with pytest.raises(RecursionError):
        engine.get_template("self.html").render(Context({}))
    assert template.render(Context({"node": tree(20)})) == expected


def test_deep_include_with_a_high_recursion_limit():
    """Rusty-only: with the limit raised, only the stack check stops a deep
    tree, and Django overflows the stack."""
    template = Engine(loaders=LOADERS).get_template("tree.html")
    limit = sys.getrecursionlimit()
    sys.setrecursionlimit(1_000_000)
    try:
        with pytest.raises(RecursionError):
            template.render(Context({"node": tree(20_000)}))
    finally:
        sys.setrecursionlimit(limit)


def test_endless_include_on_a_small_thread_stack():
    """Rusty-only: a 512 KiB stack is smaller than Django's limit allows for."""
    template = Engine(loaders=LOADERS).get_template("self.html")
    raised = []

    def render():
        try:
            template.render(Context({}))
        except RecursionError:
            raised.append(True)

    previous = threading.stack_size(512 * 1024)
    try:
        thread = threading.Thread(target=render)
        thread.start()
        thread.join()
    finally:
        threading.stack_size(previous)
    assert raised == [True]


@pytest.mark.parametrize("engine_class", ENGINES)
@pytest.mark.parametrize("width", [9_999_999_999_999_999, "9999999999999999"])
def test_center_width_too_large_to_allocate(engine_class, width):
    template = engine_class().from_string("{{ 'a'|center:width }}")
    with pytest.raises((MemoryError, OverflowError)):
        template.render(Context({"width": width}))


@pytest.mark.parametrize("width", [2_000_000, 2_000_001])
@pytest.mark.parametrize("value", ["ab", "abc"])
def test_center_width_above_the_native_limit(width, value):
    """Wider than Rusty pads natively, but small enough to allocate."""
    context = {"value": value, "width": width}
    django_output, rusty_output = (
        engine_class().from_string("{{ value|center:width }}").render(Context(context))
        for engine_class in ENGINES
    )
    assert len(rusty_output) == width
    assert rusty_output == django_output


@pytest.mark.parametrize("engine_class", ENGINES)
@pytest.mark.parametrize(
    "source",
    [
        "{% for x in xs %}{{ x|add:forloop.length }}{% endfor %}",
        "{% for x in xs %}{{ x|center:forloop.length }}{% endfor %}",
        "{% for x in xs %}{{ x|divisibleby:forloop.length }}{% endfor %}",
        "{% for x in xs %}{{ x|yesno:forloop.length }}{% endfor %}",
    ],
)
def test_forloop_length_argument(engine_class, source):
    """`forloop.length` is new in Django 6.0; before that it doesn't exist."""
    template = engine_class().from_string(source)
    context = Context({"xs": [1, 2]})
    if django.VERSION >= (6, 0):
        template.render(context)
    else:
        with pytest.raises(VariableDoesNotExist):
            template.render(context)
