"""Copying objects that hold Rusty's engine or templates works as with Django.

Django code deep-copies objects that reach the engine, such as a form's
errors (which hold the form renderer). Rusty's compiled engines and
templates don't change, so a copy is the original.
"""

import copy

import pytest
from django import forms
from django.forms.renderers import DjangoTemplates as DjangoTemplatesRenderer
from django.template import engines
from django.template.loader import get_template
from django.test import override_settings

from django_rusty_templates import Engine, PortableEngine

COPIES = [copy.copy, copy.deepcopy]


@pytest.mark.parametrize("make_copy", COPIES)
def test_copy_engine(make_copy):
    engine = Engine()
    copied = make_copy(engine)
    assert copied is engine
    assert copied.from_string("{{ x|add:1 }}").render({"x": 1}) == "2"


@pytest.mark.parametrize("make_copy", COPIES)
def test_copy_portable_engine(make_copy):
    engine = PortableEngine()
    engine.add_template("hello", "Hello {{ name }}!")
    copied = make_copy(engine)
    assert copied is engine
    assert copied.render_json("hello", '{"name": "Lily"}') == "Hello Lily!"


@pytest.mark.parametrize("make_copy", COPIES)
def test_copy_template_still_renders(make_copy):
    template = engines["rusty"].from_string("Hello {{ name }}!")
    copied = make_copy(template)
    assert copied.render({"name": "Lily"}) == "Hello Lily!"


@pytest.mark.parametrize("make_copy", COPIES)
def test_copy_origin(make_copy):
    origin = get_template("basic.txt", using="rusty").origin
    assert make_copy(origin) is origin


@pytest.mark.parametrize("make_copy", COPIES)
def test_copy_backend(make_copy):
    backend = engines["rusty"]
    copied = make_copy(backend)
    assert copied.from_string("{{ 1|add:2 }}").render({}) == "3"


class NameForm(forms.Form):
    name = forms.CharField()


class RustyRenderer(DjangoTemplatesRenderer):
    """A form renderer whose engine is Rusty's."""

    backend = type(engines["rusty"])


def test_deepcopy_form_errors():
    """Django's `FormsUtilsTestCase.test_error_dict_copy` with Rusty."""
    with override_settings(FORM_RENDERER="tests.test_copy.RustyRenderer"):
        form = NameForm(data={})
        assert type(form.renderer.engine).__name__ == "RustyTemplates"
        assert not form.is_valid()
        errors = copy.deepcopy(form.errors)
        assert errors == form.errors
        assert "This field is required." in str(errors)
