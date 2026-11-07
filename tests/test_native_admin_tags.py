"""Django admin and flatpages tags compile natively and render like Django.

The admin inclusion tags (``InclusionAdminNode``) and the token-only
``get_admin_log`` and ``get_flatpages`` compilers used to send the whole
template to Django's parser.
"""

import importlib
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace

import django
import pytest
from django.contrib.admin.templatetags import admin_list
from django.template import Context, Engine as DjangoEngine, TemplateSyntaxError
from django.test import override_settings

from django_rusty_templates import Engine

LIBRARIES = {
    "admin_list": "django.contrib.admin.templatetags.admin_list",
    "admin_modify": "django.contrib.admin.templatetags.admin_modify",
    "log": "django.contrib.admin.templatetags.log",
}

TEMPLATES = {
    "admin/change_list_object_tools.html": "global:{{ title }}",
    "admin/shop/change_form_object_tools.html": "app:{{ title }}",
    "admin/shop/item/change_list_object_tools.html": "model:{{ title }}",
    "admin/change_form_object_tools.html": "global:{{ title }}",
    "admin/actions.html": "[{{ action_index }}]",
    "admin/prepopulated_fields_js.html": (
        "{{ prepopulated_fields|length }}:{{ prepopulated_fields_json }}"
    ),
    "admin/submit_line.html": (
        "{{ show_save }} {{ show_delete_link }} {{ show_save_as_new }} "
        "{{ can_change }} {{ show_close }} {{ title }}"
    ),
    "admin/pagination.html": "pages:{{ cl }}{{ title }}",
}


def engines(**options):
    loaders = [("django.template.loaders.locmem.Loader", TEMPLATES)]
    options = {"libraries": LIBRARIES, "loaders": loaders, **options}
    return Engine(**options), DjangoEngine(**options)


def render_both(source, context):
    rusty, django_engine = engines()
    assert rusty.explain_template(source)["mode"] == "native"
    results = []
    for engine in (rusty, django_engine):
        results.append(engine.from_string(source).render(Context(dict(context))))
    return results


@pytest.fixture
def opts():
    return SimpleNamespace(app_label="Shop", model_name="item")


@pytest.mark.parametrize(
    ("tag", "expected"),
    [
        ("{% load admin_list %}{% change_list_object_tools %}", "model:Items"),
        ("{% load admin_modify %}{% change_form_object_tools %}", "app:Items"),
    ],
)
def test_object_tools_pick_the_most_specific_template(opts, tag, expected):
    rusty, django_output = render_both(tag, {"opts": opts, "title": "Items"})
    assert rusty == django_output == expected


def test_global_template_is_the_last_choice():
    opts = SimpleNamespace(app_label="other", model_name="thing")
    source = "{% load admin_list %}{% change_list_object_tools %}"
    rusty, django_output = render_both(source, {"opts": opts, "title": "T"})
    assert rusty == django_output == "global:T"


def test_admin_actions_counts_renders_in_the_live_context(opts):
    source = (
        "{% load admin_list %}{% admin_actions %}{% admin_actions %}{{ action_index }}"
    )
    rusty, django_output = render_both(source, {"opts": opts})
    assert rusty == django_output == "[0][1]1"


def test_prepopulated_fields_update_is_visible_afterwards(opts):
    source = (
        "{% load admin_modify %}{% prepopulated_fields_js %}"
        "|{{ prepopulated_fields_json }}"
    )
    rusty, django_output = render_both(source, {"opts": opts})
    assert rusty == django_output == "0:[]|[]"


def test_submit_row_wraps_the_context(opts):
    context = {
        "opts": opts,
        "title": "Edit",
        "add": False,
        "change": True,
        "is_popup": False,
        "save_as": True,
        "has_add_permission": True,
        "has_change_permission": True,
        "has_view_permission": True,
        "has_delete_permission": True,
        "has_editable_inline_admin_formsets": False,
    }
    source = "{% load admin_modify %}{% submit_row %}"
    rusty, django_output = render_both(source, context)
    assert rusty == django_output == "True True True True False Edit"


def test_function_without_context_gets_arguments(monkeypatch, opts):
    monkeypatch.setattr(admin_list, "pagination", lambda cl: {"cl": cl.upper()})
    source = "{% load admin_list %}{% pagination cl %}"
    rusty, django_output = render_both(source, {"opts": opts, "cl": "x"})
    # The child context only has the function's values, not the caller's.
    assert rusty == django_output == "pages:X"


def test_csrf_token_reaches_the_child(opts):
    rusty_engine, django_engine = engines()
    templates = {"admin/actions.html": "{% csrf_token %}"}
    loaders = [("django.template.loaders.locmem.Loader", templates)]
    for engine_class in (Engine, DjangoEngine):
        engine = engine_class(libraries=LIBRARIES, loaders=loaders)
        output = engine.from_string("{% load admin_list %}{% admin_actions %}").render(
            Context({"opts": opts, "csrf_token": "abc"})
        )
        assert 'value="abc"' in output


@pytest.mark.parametrize(
    "source",
    [
        "{% load admin_list %}{% pagination %}",
        "{% load admin_list %}{% pagination a b %}",
        "{% load admin_modify %}{% submit_row extra %}",
        "{% load admin_modify %}{% submit_row as row %}",
    ],
)
def test_argument_errors_match_django(source):
    messages = []
    for engine in engines():
        with pytest.raises(TemplateSyntaxError) as error:
            engine.from_string(source)
        messages.append(str(error.value).splitlines()[0])
    assert messages[0] == messages[1]


def test_missing_opts_raises_key_error():
    for engine in engines():
        template = engine.from_string("{% load admin_list %}{% admin_actions %}")
        with pytest.raises(KeyError, match="opts"):
            template.render(Context({}))


def test_replacement_compiler_is_not_reinterpreted(monkeypatch):
    module = ModuleType("replacement_admin_tags")
    from django.template import Library
    from django.template.base import TextNode

    module.register = Library()

    @module.register.tag("admin_actions")
    def admin_actions_tag(parser, token):
        return TextNode("replaced")

    monkeypatch.setitem(sys.modules, module.__name__, module)
    rusty = Engine(libraries={"admin_list": module.__name__})
    assert rusty.explain_template("{% load admin_list %}{% admin_actions %}")[
        "mode"
    ] == ("hybrid")
    assert (
        rusty.from_string("{% load admin_list %}{% admin_actions %}").render(
            Context({})
        )
        == "replaced"
    )


class Entries:
    def __init__(self, entries):
        self.entries = entries

    def filter(self, user__pk):
        return Entries([entry for entry in self.entries if entry[0] == int(user__pk)])

    def __getitem__(self, index):
        return [entry[1] for entry in self.entries][index]


@pytest.mark.parametrize(
    ("arguments", "expected"),
    [
        ("2 as log", "a,b,"),
        ("5 as log for_user 2", "b,c,"),
        ("5 as log for_user user", "a,"),
    ],
)
def test_get_admin_log(arguments, expected):
    entries = Entries([(1, "a"), (2, "b"), (2, "c"), (3, "d")])
    source = (
        f"{{% load log %}}{{% get_admin_log {arguments} %}}"
        "{% for entry in log %}{{ entry }},{% endfor %}"
    )
    context = {"log_entries": entries, "user": SimpleNamespace(pk=1)}
    rusty, django_output = render_both(source, context)
    assert rusty == django_output == expected


@pytest.mark.parametrize(
    "arguments",
    ["", "2 as", "x as log", "2 to log", "2 as log for user"],
)
def test_get_admin_log_errors_match_django(arguments):
    source = f"{{% load log %}}{{% get_admin_log {arguments} %}}"
    messages = []
    for engine in engines():
        with pytest.raises(TemplateSyntaxError) as error:
            engine.from_string(source)
        messages.append(str(error.value).splitlines()[0])
    assert messages[0] == messages[1]


class Pages:
    def __init__(self, filters=()):
        self.filters = filters

    def filter(self, **kwargs):
        return Pages((*self.filters, *sorted(kwargs.items())))

    def __iter__(self):
        return iter(f"{key}={value}" for key, value in self.filters)


@pytest.fixture
def flatpages(monkeypatch):
    """The flatpages library, with its model replaced by a recording stub.

    The test settings don't install ``django.contrib.sites``.
    """
    models = ModuleType("django.contrib.flatpages.models")
    models.FlatPage = SimpleNamespace(objects=Pages())
    name = "django.contrib.flatpages.templatetags.flatpages"
    monkeypatch.setitem(sys.modules, models.__name__, models)
    monkeypatch.delitem(sys.modules, name, raising=False)
    importlib.import_module(name)
    yield {"flatpages": name}
    sys.modules.pop(name, None)


@pytest.mark.parametrize(
    ("arguments", "expected"),
    [
        ("as pages", "sites__id=7,registration_required=False,"),
        (
            "'/about/' for user as pages",
            "sites__id=7,url__startswith=/about/,",
        ),
        (
            "prefix as pages",
            "sites__id=7,url__startswith=/p/,registration_required=False,",
        ),
    ],
)
def test_get_flatpages(flatpages, arguments, expected):
    source = (
        f"{{% load flatpages %}}{{% get_flatpages {arguments} %}}"
        "{% for page in pages %}{{ page }},{% endfor %}"
    )
    context = {"prefix": "/p/", "user": SimpleNamespace(is_authenticated=True)}
    outputs = []
    with override_settings(SITE_ID=7):
        for engine_class in (Engine, DjangoEngine):
            engine = engine_class(libraries=flatpages)
            if engine_class is Engine:
                assert engine.explain_template(source)["mode"] == "native"
            outputs.append(engine.from_string(source).render(Context(context)))
    assert outputs[0] == outputs[1] == expected


@pytest.mark.skipif(
    not (Path(django.__file__).parent / "contrib/admin/templates").exists(),
    reason="Django's admin templates are not installed",
)
def test_contrib_admin_pages_compile_natively():
    admin = Path(django.__file__).parent / "contrib/admin/templates"
    rusty = Engine(
        dirs=[str(admin)],
        libraries={
            **LIBRARIES,
            "admin_urls": "django.contrib.admin.templatetags.admin_urls",
            "i18n": "django.templatetags.i18n",
            "static": "django.templatetags.static",
        },
    )
    for name in ("change_form.html", "change_list.html", "index.html"):
        source = (admin / "admin" / name).read_text()
        assert rusty.explain_template(source, f"admin/{name}")["mode"] == "native"
