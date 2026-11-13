"""The `isolated_tags` engine option opts a library's tags into isolated
compilation, as if each compiler set `rusty_isolated_compiler` (#360)."""

import sys
from types import ModuleType

import pytest
from django.core.exceptions import ImproperlyConfigured
from django.template import Context, Engine as DjangoEngine, Library, Node

from django_rusty_templates import Engine, RustyTemplates

MODULE = "rusty_isolated_tags_option_tests"
OTHER = "rusty_isolated_tags_option_other"
LIBRARIES = {"cards": MODULE, "other": OTHER}
SOURCE = (
    "{% load cards %}<ul>{% for row in rows %}<li>{{ row }}</li>{% endfor %}</ul>"
    "{% card row %}<b>{{ row|upper }}</b>{% endcard %}"
)


class CardNode(Node):
    child_nodelists = ("nodelist",)

    def __init__(self, title, nodelist):
        self.title = title
        self.nodelist = nodelist

    def render(self, context):
        return f"[{self.title.resolve(context)}:{self.nodelist.render(context)}]"


class MarkNode(Node):
    def render(self, context):
        return "|"


@pytest.fixture(autouse=True)
def calls(monkeypatch):
    calls = []
    module = ModuleType(MODULE)
    module.register = Library()

    @module.register.tag("card")
    def card(parser, token):
        calls.append(token.contents)
        title = parser.compile_filter(token.split_contents()[1])
        nodelist = parser.parse(("endcard",))
        parser.delete_first_token()
        return CardNode(title, nodelist)

    @module.register.tag("full")
    def full(parser, token):
        return MarkNode()

    full.rusty_requires_full_parser = True

    other = ModuleType(OTHER)
    other.register = Library()

    @other.register.tag("mark")
    def mark(parser, token):
        calls.append(token.contents)
        return MarkNode()

    monkeypatch.setitem(sys.modules, MODULE, module)
    monkeypatch.setitem(sys.modules, OTHER, other)
    return calls


def values():
    return {"rows": ["a", "<b>"], "row": "x"}


def test_listed_tags_compile_natively_and_render_like_django(calls):
    expected = DjangoEngine(libraries=LIBRARIES).from_string(SOURCE)
    calls.clear()
    engine = Engine(libraries=LIBRARIES, isolated_tags={"cards": ["card"]})
    template = engine.from_string(SOURCE)
    assert template._is_native
    assert calls == ["card row"]
    assert template.render(values()) == expected.render(Context(values()))


def test_opt_in_belongs_to_the_engine(calls):
    Engine(libraries=LIBRARIES, isolated_tags={"cards": ["card"]})
    assert not Engine(libraries=LIBRARIES).from_string(SOURCE)._is_native
    card = sys.modules[MODULE].register.tags["card"]
    assert not hasattr(card, "rusty_isolated_compiler")


def test_unlisted_tags_still_use_djangos_parser(calls):
    source = "{% load cards other %}{% card row %}{% mark %}{% endcard %}"
    engine = Engine(libraries=LIBRARIES, isolated_tags={"cards": ["card"]})
    calls.clear()
    template = engine.from_string(source)
    assert not template._is_native
    # The listed compiler runs once, on Django's parser, not twice.
    assert calls == ["card row", "mark"]
    expected = DjangoEngine(libraries=LIBRARIES).from_string(source)
    assert template.render(values()) == expected.render(Context(values()))


def test_builtins_are_named_by_module_path(calls):
    source = "{% card row %}{{ row }}{% endcard %}"
    engine = Engine(builtins=[MODULE], isolated_tags={MODULE: ["card"]})
    assert engine.from_string(source)._is_native


def test_backend_passes_the_option_through(calls, monkeypatch):
    # The test settings install a library without tags, which keeps every
    # custom compiler on Django's parser.
    monkeypatch.setattr("django_rusty_templates.get_installed_libraries", dict)
    backend = RustyTemplates(
        {
            "NAME": "rusty",
            "DIRS": [],
            "APP_DIRS": False,
            "OPTIONS": {"libraries": LIBRARIES, "isolated_tags": {"cards": ["card"]}},
        }
    )
    assert backend.from_string(SOURCE)._is_native


@pytest.mark.parametrize(
    ("isolated_tags", "message"),
    [
        (
            {"missing": ["card"]},
            "isolated_tags names the template library 'missing', which isn't one "
            "of this engine's libraries or builtins.",
        ),
        (
            {"cards": ["crad"]},
            "isolated_tags names the tag 'crad', which the template library "
            "'cards' doesn't register.",
        ),
        (
            {"cards": "card"},
            "isolated_tags['cards'] must be a list of tag names, not a string.",
        ),
        (
            {"cards": ["full"]},
            "The 'full' tag in 'cards' sets rusty_requires_full_parser, so "
            "isolated_tags can't opt it in.",
        ),
    ],
)
def test_names_that_dont_exist_are_rejected(isolated_tags, message):
    with pytest.raises(ImproperlyConfigured) as info:
        Engine(libraries=LIBRARIES, isolated_tags=isolated_tags)
    assert str(info.value) == message
