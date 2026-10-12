from datetime import datetime

import django
import pytest
import time_machine
from django.template import Context, Engine as DjangoEngine
from django.utils.safestring import mark_safe

from django_rusty_templates import Engine, interop

pytestmark = pytest.mark.skipif(
    django.VERSION < (6, 2), reason="Now expressions require Django 6.2"
)


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize(
    "source,context",
    [
        ('{% now "Y" %}', {}),
        ("{% now fmt %}", {"fmt": "j n Y"}),
        ("{% now fmt as result %}{{ result }}", {"fmt": r"\<\b\>"}),
        ('{% now ""|add:fmt %}', {"fmt": r"\<\b\>"}),
        ("{% now missing %}", {}),
        ("{% now fmt %}", {"fmt": None}),
        ("{% now fmt %}", {"fmt": mark_safe(r"\<\b\>")}),
        ("{% autoescape off %}{% now fmt %}{% endautoescape %}", {"fmt": r"\<\b\>"}),
    ],
)
@time_machine.travel(datetime(2026, 1, 8, 12), tick=False)
def test_now_expressions_compile_natively(monkeypatch, debug, source, context):
    options = dict(
        debug=debug,
        loaders=[("django.template.loaders.locmem.Loader", {"page.html": source})],
    )
    expected = DjangoEngine(**options).from_string(source).render(Context(context))
    engine = Engine(**options)

    def unexpected(*args, **kwargs):
        raise AssertionError("now expressions used Django parsing")

    monkeypatch.setattr(interop, "compile_template", unexpected)
    assert engine.explain_template(source)["mode"] == "native"
    assert engine.from_string(source).render(context) == expected
    assert engine.get_template("page.html").render(context) == expected
