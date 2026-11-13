"""Shortcuts that keep common cases out of Python (#417). Each one renders
the same as Django, including where the shortcut must not apply."""

import pytest
from django.template import Context, Engine as DjangoEngine
from django.utils import html
from django.utils.safestring import SafeString, mark_safe

from django_rusty_templates import Engine

OPTIONS = {"libraries": {"i18n": "django.templatetags.i18n"}}


class Plain:
    def __str__(self):
        return "<plain>"


class Html:
    def __str__(self):
        return "<str>"

    def __html__(self):
        return "<html>"


class SafeText:
    def __str__(self):
        return mark_safe("<safe>")


class TextWithHtml(str):
    def __html__(self):
        return "<subclass>"


class StrSubclassText:
    def __str__(self):
        return TextWithHtml("<text>")


def values():
    return {
        "yes": True,
        "no": False,
        "none": None,
        "plain": Plain(),
        "html": Html(),
        "safe_text": SafeText(),
        "subclass_text": StrSubclassText(),
        "safe": SafeString("<b>"),
        "items": [1, None, Plain(), Html()],
        "long": "aé😀<" * 100,
        "short": "aé😀<",
        "mail": "x@example.com, see https://example.com/a?b=1&c=2.",
    }


SOURCES = [
    # `yesno` translates its default once per language.
    "{{ yes|yesno }}{{ no|yesno }}{{ none|yesno }}",
    "{% load i18n %}{{ yes|yesno }}{% language 'de' %}{{ yes|yesno }}{{ none|yesno }}"
    "{% endlanguage %}{{ no|yesno }}{% language 'fr' %}{{ no|yesno }}{% endlanguage %}",
    "{% load i18n %}{% for i in items %}{% language 'de' %}{{ yes|yesno }}"
    "{% endlanguage %}{{ yes|yesno }}{% endfor %}",
    "{{ yes|yesno:none }}{{ yes|yesno:'a,b' }}",
    # Printing a value that isn't a string.
    "{{ none }}{{ plain }}{{ html }}{{ safe_text }}{{ subclass_text }}{{ safe }}",
    "{% autoescape off %}{{ none }}{{ plain }}{{ html }}{{ safe_text }}"
    "{{ subclass_text }}{% endautoescape %}",
    "{% for i in items %}{{ i }}|{% endfor %}",
    "{{ plain|default:'d' }}{{ html|default:'d' }}",
    # `make_list`, short and long.
    "{{ long|make_list|length }}{{ long|make_list|first }}{{ long|make_list|last }}",
    "{{ short|make_list }}{{ long|make_list|slice:':5' }}{{ 12345|make_list }}",
    # The active language.
    "{% load i18n %}{% get_current_language as a %}{{ a }}{% language 'de' %}"
    "{% get_current_language as b %}{{ b }}{% endlanguage %}"
    "{% get_current_language as c %}{{ c }}",
    # `urlize` checks Django's `urlizer` once per render.
    "{{ mail|urlize }}{{ mail|urlize }}{% for i in items %}{{ mail|urlizetrunc:9 }}"
    "{% endfor %}",
]


def render_both(source):
    expected = DjangoEngine(**OPTIONS).from_string(source).render(Context(values()))
    return Engine(**OPTIONS).from_string(source).render(values()), expected


@pytest.mark.parametrize("source", SOURCES)
def test_matches_django(source):
    rendered, expected = render_both(source)
    assert rendered == expected


def test_a_customized_urlizer_is_seen_by_the_next_render(monkeypatch):
    source = "{{ mail|urlize }}"
    template = Engine().from_string(source)
    before = template.render(values())
    assert before == DjangoEngine().from_string(source).render(Context(values()))

    monkeypatch.setattr(
        html.urlizer, "url_template", '<a class="x" href="{href}">{url}</a>'
    )
    after = template.render(values())
    assert after == DjangoEngine().from_string(source).render(Context(values()))
    assert after != before
