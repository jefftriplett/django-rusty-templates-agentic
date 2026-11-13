"""The context a `takes_context` simple tag receives supports the parts of
Django's `Context` API that tags use (#415). Each template renders the same
through Django."""

import copy
import sys
from types import ModuleType

import pytest
from django.template import Context, Engine as DjangoEngine, Library, RequestContext
from django.test import RequestFactory
from django.template.context import ContextPopException

from django_rusty_templates import Engine

MODULE = "rusty_takes_context_api_tags"


def build_library():
    register = Library()

    @register.simple_tag(takes_context=True)
    def show(context, name):
        return f"[{context.get(name, 'missing')}]"

    @register.simple_tag(takes_context=True)
    def push_with(context, **values):
        with context.push(**values):
            inside = [context.get(name) for name in values]
        return f"{inside}"

    @register.simple_tag(takes_context=True)
    def push_mapping(context):
        with context.push({"u": 1}, v=2):
            return f"{context['u']}{context['v']}"

    @register.simple_tag(takes_context=True)
    def push_assign(context):
        with context.push():
            context["u"] = "inner"
            inside = context["u"]
        return f"{inside}"

    @register.simple_tag(takes_context=True)
    def push_pop(context, value):
        context.push()
        context["u"] = value
        inside = context["u"]
        context.pop()
        return f"{inside}"

    @register.simple_tag(takes_context=True)
    def push_stay(context, value):
        context.push()
        context["u"] = value
        return ""

    @register.simple_tag(takes_context=True)
    def push_nested(context):
        with context.push(u=1):
            with context.push(u=2):
                inner = context["u"]
            middle = context["u"]
        return f"{inner}{middle}"

    @register.simple_tag(takes_context=True)
    def update_with(context, value):
        with context.update({"u": value}):
            inside = context["u"]
        return f"{inside}"

    @register.simple_tag(takes_context=True)
    def update_stay(context, value):
        context.update({"u": value})
        return ""

    @register.simple_tag(takes_context=True)
    def default(context, name, value):
        return f"{context.setdefault(name, value)}"

    @register.simple_tag(takes_context=True)
    def upward(context, name, value):
        with context.push():
            context.set_upward(name, value)
        return ""

    @register.simple_tag(takes_context=True)
    def count(context):
        context.render_context["count"] = context.render_context.get("count", 0) + 1
        return f"{context.render_context['count']}"

    @register.simple_tag(takes_context=True)
    def has(context, name):
        return f"{name in context}"

    @register.simple_tag(takes_context=True)
    def loop_counter(context):
        return f"{context['forloop']['counter']}{'forloop' in context.flatten()}"

    @register.simple_tag(takes_context=True)
    def fresh(context):
        new = context.new({"only": 1})
        return f"{'only' in new}{'name' in new}{new.autoescape}{'True' in new}"

    @register.simple_tag(takes_context=True)
    def copied(context):
        duplicate = copy.copy(context)
        duplicate["u"] = "copy"
        return f"{duplicate['u']}{context.get('u', 'none')}"

    @register.simple_tag(takes_context=True)
    def pop_too_far(context):
        try:
            context.pop()
            context.pop()
            context.pop()
            context.pop()
        except ContextPopException:
            return "refused"
        return "popped"

    @register.simple_tag(takes_context=True)
    def describe_template(context):
        template = context.template
        inline = template.engine.from_string("[{{ name }}]").render(context)
        loaded = template.engine.get_template("included.html").render(context)
        return f"{context.template_name}|{template.name}|{inline}|{loaded}"

    @register.simple_tag(takes_context=True)
    def context_kind(context):
        is_context = isinstance(context, Context)
        is_request = isinstance(context, RequestContext)
        # The check packages make before rendering a backend template.
        values = context.flatten() if is_context else context
        return f"{is_context}|{is_request}|{type(values).__name__}"

    @register.simple_tag(takes_context=True)
    def layers(context):
        merged = {}
        for layer in context.dicts:
            merged.update(layer)
        builtins = sorted(context.dicts[0])
        return f"{builtins}|{merged['name']}|{merged == context.flatten()}"

    return register


@pytest.fixture(autouse=True)
def library(monkeypatch):
    module = ModuleType(MODULE)
    module.register = build_library()
    monkeypatch.setitem(sys.modules, MODULE, module)


SOURCES = [
    "{% push_with u=1 v=2 %}{% show 'u' %}{% show 'v' %}",
    "{% push_with name='tag' %}{% show 'name' %}",
    "{% push_mapping %}{% show 'u' %}{% show 'v' %}",
    "{% push_assign %}{% show 'u' %}",
    "{% push_pop 'x' %}{% show 'u' %}",
    "{% push_nested %}{% show 'u' %}",
    "{% update_with 'x' %}{% show 'u' %}",
    "{% update_with 'x' %}{{ name }}",
    # A scope a tag leaves open is the next one popped, by whatever closes.
    "{% push_stay 'x' %}{% show 'u' %}{{ u }}",
    "{% update_stay 'x' %}{% show 'u' %}{{ u }}",
    "{% for i in items %}{% update_stay i %}{{ u }}{% endfor %}[{{ u }}]",
    "{% for i in items %}{% push_stay i %}{{ u }}{% endfor %}[{{ u }}]",
    "{% with u='w' %}{% push_stay 'x' %}{{ u }}{% endwith %}[{{ u }}]",
    "{% with u='w' %}{% update_stay 'x' %}{% update_stay 'y' %}{{ u }}{% endwith %}"
    "[{{ u }}]",
    "{% for i in items %}{% with u='w' %}{% push_stay i %}{{ u }}{% endwith %}"
    "{{ u }}{% endfor %}",
    "{% update_stay 'x' %}{% for i in items %}{% update_stay i %}{% endfor %}{{ u }}",
    # setdefault and set_upward.
    "{% default 'name' 'd' %}{% default 'new' 'd' %}{{ new }}{% show 'new' %}",
    "{% upward 'name' 'up' %}{{ name }}{% upward 'fresh' 'up' %}[{{ fresh }}]",
    "{% with name='w' %}{% upward 'name' 'up' %}{{ name }}{% endwith %}{{ name }}",
    # render_context lasts for the render.
    "{% count %}{% count %}{% for i in items %}{% count %}{% endfor %}",
    # Lookups.
    "{% has 'name' %}{% has 'missing' %}{% has 'forloop' %}"
    "{% for i in items %}{% has 'forloop' %}{% loop_counter %}{% endfor %}",
    "{% fresh %}",
    "{% copied %}{% show 'u' %}",
]


def values():
    return {"name": "caller", "items": [1, 2]}


@pytest.mark.parametrize("source", SOURCES)
def test_tag_context_matches_django(source):
    source = "{% load api %}" + source
    options = {"libraries": {"api": MODULE}}
    expected = DjangoEngine(**options).from_string(source).render(Context(values()))
    assert Engine(**options).from_string(source).render(values()) == expected


def test_render_twice_starts_a_new_render_context():
    template = Engine(libraries={"api": MODULE}).from_string(
        "{% load api %}{% count %}{% count %}"
    )
    assert template.render({}) == "12"
    assert template.render({}) == "12"


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("{% pop_too_far %}{{ name }}", "refusedcaller"),
        (
            "{% for i in items %}{% pop_too_far %}{{ i }}{% endfor %}",
            "refused1refused2",
        ),
        ("{% with u='w' %}{% pop_too_far %}{{ u }}{% endwith %}", "refusedw"),
    ],
)
def test_a_tag_cant_pop_scopes_it_didnt_push(source, expected):
    # Django lets a tag pop the template's own layers, down to the caller's
    # values. Rusty refuses at the first pop the tag didn't push.
    template = Engine(libraries={"api": MODULE}).from_string("{% load api %}" + source)
    assert template.render(values()) == expected


TEMPLATES = {
    "page.html": "{% load api %}{% describe_template %}{% include 'child.html' %}",
    "child.html": "{% load api %} child:{% describe_template %}",
    "included.html": "<{{ name }}>",
}


def template_engines():
    options = {
        "libraries": {"api": MODULE},
        "loaders": [("django.template.loaders.locmem.Loader", TEMPLATES)],
    }
    return DjangoEngine(**options), Engine(**options)


def test_context_template_is_the_template_being_rendered():
    django, rusty = template_engines()
    expected = django.get_template("page.html").render(Context(values()))
    actual = rusty.get_template("page.html").render(Context(values()))
    assert actual == expected
    assert actual == (
        "page.html|page.html|[caller]|&lt;caller&gt;"
        " child:page.html|page.html|[caller]|&lt;caller&gt;"
    )


def test_context_template_of_a_string_template_has_no_name():
    django, rusty = template_engines()
    source = "{% load api %}{% describe_template %}"
    expected = django.from_string(source).render(Context(values()))
    actual = rusty.from_string(source).render(Context(values()))
    assert actual == expected == "None|None|[caller]|&lt;caller&gt;"


@pytest.mark.parametrize("with_request", [False, True])
def test_context_is_an_instance_of_djangos_context(with_request):
    def context():
        if with_request:
            return RequestContext(RequestFactory().get("/"), values())
        return Context(values())

    django, rusty = template_engines()
    source = "{% load api %}{% context_kind %}"
    expected = django.from_string(source).render(context())
    actual = rusty.from_string(source).render(context())
    assert actual == expected == f"True|{with_request}|dict"


def test_context_dicts_lists_the_variables():
    django, rusty = template_engines()
    source = "{% load api %}{% with name='inner' %}{% layers %}{% endwith %}"
    expected = django.from_string(source).render(Context(values()))
    actual = rusty.from_string(source).render(Context(values()))
    assert actual == expected
    assert actual.endswith("|inner|True")
