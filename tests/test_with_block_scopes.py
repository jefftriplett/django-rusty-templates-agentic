"""`{% with %}` and `{% block %}` bind their names in a scope on the render's
own context, as `{% for %}` does, and what their bodies look up stays cached
for the next one (#416). Each template renders the same as Django."""

import datetime

import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine

TEMPLATES = {
    "base.html": "{% block a %}A{{ z }}{% endblock %}|{% block b %}B{% endblock %}",
    "child.html": (
        "{% extends 'base.html' %}"
        "{% block a %}{% with z=2 %}{{ block.super }}{{ z }}{% endwith %}[{{ z }}]"
        "{% now 'Y' as year %}{{ year }}{% endblock %}"
        "{% block b %}[{{ year }}]{{ block.super }}{% endblock %}"
    ),
    "date.html": "{{ row.d|date:'D d M Y' }}",
}
OPTIONS = {
    "loaders": [("django.template.loaders.locmem.Loader", TEMPLATES)],
    "libraries": {"i18n": "django.templatetags.i18n"},
}

SOURCES = [
    # The names end with the scope.
    "{% with a=1 %}{{ a }}{% endwith %}[{{ a }}]",
    "{% with name='w' %}{{ name }}{% endwith %}[{{ name }}]",
    "{% with a=1 a=2 %}{{ a }}{% endwith %}[{{ a }}]",
    "{% with name='w' name='x' %}{{ name }}{% endwith %}[{{ name }}]",
    "{% with a=1 %}{% with a=2 %}{{ a }}{% endwith %}{{ a }}{% endwith %}[{{ a }}]",
    "{% with a=name b=a %}{{ a }}{{ b }}{% endwith %}",
    "{% with a=missing %}[{{ a }}]{% endwith %}",
    # What the body assigns ends with it too.
    "{% with a=1 %}{% now 'Y' as year %}{{ year }}{% endwith %}[{{ year }}]",
    "{% with a=1 %}{% cycle 'x' 'y' as c %}{% endwith %}[{{ c }}]",
    "{% cycle 'x' 'y' as c %}{% with a=1 %}{% cycle c %}{% endwith %}[{{ c }}]",
    "{% with c='w' %}{% cycle 'x' 'y' as c %}{{ c }}{% endwith %}[{{ c }}]",
    "{% with a=1 %}{% firstof name 'x' as name %}{{ name }}{% endwith %}[{{ name }}]",
    "{% with name='w' %}{% firstof 'x' as name %}{{ name }}{% endwith %}[{{ name }}]",
    # Inside loops, and a loop inside the scope.
    "{% for i in items %}{% with a=i %}{{ a }}{{ forloop.counter }}{% endwith %}"
    "[{{ a }}]{% endfor %}",
    "{% with i='w' %}{% for i in items %}{{ i }}{% endfor %}{{ i }}{% endwith %}",
    "{% for i in items %}{% with i='w' %}{{ i }}{% endwith %}{{ i }}{% endfor %}",
    "{% for i in items %}{% with a=1 %}{% ifchanged %}c{% endifchanged %}"
    "{% cycle 'x' 'y' %}{% endwith %}{% endfor %}",
    # The caller's dict is still read live around a scope.
    "{% with name='w' %}{{ change }}{{ name }}{% endwith %}|{{ name }}",
    "{% with name='w' %}{{ name }}{% endwith %}{{ change }}|{{ name }}",
    "{% with name='w' %}{{ drop }}{{ name }}{% endwith %}|[{{ name }}]",
    "{% for name in items %}{{ name }}{% endfor %}{{ change }}|{{ name }}",
    "{% for name in items %}{{ change }}{% endfor %}|{{ name }}",
    "{% with name='w' %}{% with name='x' %}{{ change }}{% endwith %}{{ name }}"
    "{% endwith %}|{{ name }}",
    "{% with late='w' %}{{ add }}{{ late }}{% endwith %}|{{ late }}",
    "{% with late='w' %}{% with late='x' %}{{ add }}{% endwith %}{{ late }}"
    "{% endwith %}|{{ late }}",
    "{% with late='w' %}{% firstof 'x' as late %}{{ add }}{{ late }}{% endwith %}"
    "|{{ late }}",
    "{% with name='w' %}{% firstof 'x' as name %}{{ change }}{{ name }}{% endwith %}"
    "|{{ name }}",
    "{% for late in items %}{{ add }}{% endfor %}|{{ late }}",
    # Blocks.
    "{% block a %}{{ name }}{% endblock %}[{{ block }}]",
    "{% block a %}{% now 'Y' as year %}{{ year }}{% endblock %}[{{ year }}]",
    "{% for i in items %}{% block a %}{{ i }}{% cycle 'x' 'y' %}{% endblock %}"
    "{% endfor %}",
    "{% block a %}{% with block='w' %}{{ block }}{% endwith %}{% endblock %}",
    "{% extends 'child.html' %}",
    # Scopes keep the render's caches.
    "{% for row in rows %}{% with z=1 %}{{ row.d|date:'D d M Y' }}{% endwith %}"
    "{% block a %}{{ row.d|date:'N j' }}{% endblock %}"
    "{% include 'date.html' with row=row only %}{% endfor %}",
    "{% for row in rows %}{% with z=1 %}{% language 'de' %}{{ row.d|date:'D F' }}"
    "{% endlanguage %}{{ row.d|date:'D F' }}{% endwith %}{% endfor %}",
    "{% for row in rows %}{% language 'de' %}{% with z=1 %}{{ row.d|date:'D F' }}"
    "{% endwith %}{% include 'date.html' with row=row only %}{% endlanguage %}"
    "{{ row.d|date:'D F' }}{% endfor %}",
]


def build():
    context = {
        "name": "caller",
        "items": [1, 2, 2],
        "rows": [{"d": datetime.date(2026, 5, day)} for day in (4, 5)],
    }
    context["add"] = lambda: context.update(late="added") or ""
    context["change"] = lambda: context.update(name="changed") or ""
    context["drop"] = lambda: context.pop("name", "") and ""
    return context


@pytest.mark.parametrize("source", SOURCES)
def test_scopes_render_like_django(source, time_machine):
    time_machine.move_to("2026-05-04 12:00:00")
    source = "{% load i18n %}" + source if "language" in source else source
    expected = DjangoEngine(**OPTIONS).from_string(source).render(Context(build()))
    assert Engine(**OPTIONS).from_string(source).render(build()) == expected


def test_the_scope_closes_when_its_body_raises():
    def boom():
        raise ValueError("boom")

    template = Engine().from_string(
        "{% for i in items %}{% with a=i %}{{ boom }}{% endwith %}{% endfor %}"
    )
    context = {"items": [1], "boom": boom}
    for _ in range(2):
        with pytest.raises(ValueError, match="boom"):
            template.render(context)
    assert context == {"items": [1], "boom": boom}
