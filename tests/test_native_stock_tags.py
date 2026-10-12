from datetime import datetime, timezone
from decimal import Decimal

import pytest
from django.core.cache import caches
from django.template import Context, Engine as DjangoEngine
from django.template.base import Parser

from django_rusty_templates import Engine, interop

LIBRARIES = {
    name: "django.templatetags." + name
    for name in ["static", "cache", "l10n", "tz", "i18n"]
}


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("cached", [False, True])
@pytest.mark.parametrize(
    "source",
    [
        "{% load static %}{% static path %}|{% static path as url %}{{ url }}|{% get_static_prefix %}|{% get_media_prefix as media %}{{ media }}",
        "{% load cache %}{% cache timeout native-stock-test value using=cache %}{{ value }}{% firstof value as assigned %}{% endcache %}{{ assigned }}",
        "{% load l10n %}{% localize off %}{{ number }}{% endlocalize %}|{% localize on %}{{ number }}{% endlocalize %}",
        '{% load tz %}{% timezone "Europe/Paris" %}{% localtime on %}{{ when }}{% endlocaltime %}{% get_current_timezone as zone %}{{ zone }}{% endtimezone %}',
        '{% load i18n %}{% language "de" %}{% get_current_language as lang %}{{ lang }}{% get_current_language_bidi as bidi %}{{ bidi }}{% endlanguage %}',
        '{% load i18n %}{% get_available_languages as available %}{% get_language_info for "fr" as info %}{{ info.code }}{% get_language_info_list for languages as infos %}{% for info in infos %}{{ info.code }}{% endfor %}',
        '{% load i18n %}{% translate "Hello" %}|{% trans value noop as translated %}{{ translated }}|{% translate "50%" context "percentage" %}',
        '{% load i18n %}{% language "de" %}{% translate "January"|upper %}|{% blocktranslate trimmed with item=value %} Hello {{ item }}! {% endblocktranslate %}{% endlanguage %}',
        '{% load i18n %}{% blocktranslate count count=amount context "cards" asvar result %}{{ count }} item: {{ value }}{% plural %}{{ count }} items: {{ value }}{% endblocktranslate %}{{ result }}',
        "{% load i18n %}{% blocktrans with value as item %}{{ item }} 50%{% endblocktrans %}|{{ item }}",
        "{% load i18n %}{% blocktranslate with item=missing item=value %}{{ item }}{% endblocktranslate %}",
        "{% load i18n %}{% blocktranslate count count=missing %}{{ count }}{% plural %}{% endblocktranslate %}",
    ],
)
def test_stock_tags_do_not_invoke_django_parser(
    monkeypatch, settings, debug, cached, source
):
    settings.STATIC_URL = "/static/"
    settings.MEDIA_URL = "/media/"
    loader = ("django.template.loaders.locmem.Loader", {"page.html": source})
    options = dict(
        debug=debug,
        libraries=LIBRARIES,
        loaders=[("django.template.loaders.cached.Loader", [loader])]
        if cached
        else [loader],
    )
    values = dict(
        value="<&>",
        path="app/a&b.css",
        timeout=60,
        cache="default",
        number=Decimal("1234.5"),
        when=datetime(2026, 1, 8, 12, tzinfo=timezone.utc),
        languages=["en", "fr"],
        amount=2,
    )
    caches["default"].clear()
    expected = DjangoEngine(**options).from_string(source).render(Context(values))
    engine = Engine(**options)

    def unexpected(*args, **kwargs):
        raise AssertionError("stock tag entered Django parsing")

    monkeypatch.setattr(interop, "compile_template", unexpected)
    monkeypatch.setattr(Parser, "parse", unexpected)
    assert engine.explain_template(source)["mode"] == "native"
    for load in [
        lambda: engine.from_string(source),
        lambda: engine.get_template("page.html"),
    ]:
        caches["default"].clear()
        assert load().render(Context(values)) == expected


def test_stock_compiler_identity_preserves_custom_replacements(monkeypatch):
    import sys
    from types import ModuleType
    from django.template import Library, Node

    module = ModuleType("rusty_replaced_static")
    module.register = Library()
    monkeypatch.setitem(sys.modules, module.__name__, module)

    class Replacement(Node):
        def render(self, context):
            return "replacement"

    def replacement(parser, token):
        return Replacement()

    replacement.__module__ = "django.templatetags.static"
    replacement.__name__ = "do_static"
    module.register.tag("static", replacement)
    engine = Engine(libraries={"custom": module.__name__})
    source = "{% load custom %}{% static %}"
    assert engine.explain_template(source)["mode"] == "hybrid"
    assert engine.from_string(source).render() == "replacement"


@pytest.mark.parametrize(
    "source",
    [
        '{% load i18n %}{% language "fr" %}{{ broken }}{% endlanguage %}',
        '{% load tz %}{% timezone "Europe/Paris" %}{{ broken }}{% endtimezone %}',
        "{% load l10n %}{% localize off %}{{ broken }}{% endlocalize %}",
        "{% load tz %}{% localtime off %}{{ broken }}{% endlocaltime %}",
        "{% load i18n %}{% blocktranslate with item=broken %}{{ item }}{% endblocktranslate %}",
    ],
)
def test_stock_scope_restores_after_render_error(source):
    from django.utils import timezone as django_timezone, translation

    failure = RuntimeError("stock body failed")

    class Broken:
        def __str__(self):
            raise failure

    context = Context({"broken": Broken()}, use_l10n=True, use_tz=True)
    dictionaries = list(context.dicts)
    language = translation.get_language()
    zone = django_timezone.get_current_timezone()
    template = Engine(libraries=LIBRARIES).from_string(source)
    with pytest.raises(RuntimeError) as caught:
        template.render(context)
    assert caught.value is failure
    assert context.dicts == dictionaries
    assert context.use_l10n is True
    assert context.use_tz is True
    assert context.template is None
    assert translation.get_language() == language
    assert django_timezone.get_current_timezone() == zone


def test_native_cache_hit_skips_body_assignments():
    source = "{% load cache %}{% cache 60 native-stock-hit value %}{{ value }}{% firstof value as assigned %}{% endcache %}|{{ assigned }}"
    outputs = []
    for kind in [DjangoEngine, Engine]:
        caches["default"].clear()
        template = kind(libraries=LIBRARIES).from_string(source)
        outputs.append([template.render(Context({"value": "<&>"})) for _ in range(2)])
    assert outputs[0] == outputs[1]


def test_stock_cache_inspection_preserves_django_nodes():
    from django.templatetags.cache import CacheNode

    template = Engine(libraries=LIBRARIES).from_string(
        "{% load cache %}{% cache 1 component %}content{% endcache %}"
    )
    assert template._is_native
    nodes = template.nodelist.get_nodes_by_type(CacheNode)
    assert len(nodes) == 1
    assert nodes[0].fragment_name == "component"


@pytest.mark.parametrize("options", ["", "noop", 'context "different"'])
def test_translated_literal_is_not_translated_twice(monkeypatch, options):
    from django.utils import translation

    def gettext(message):
        return {"original": "translated", "translated": "translated twice"}.get(
            message, message
        )

    monkeypatch.setattr(translation._trans, "gettext", gettext)
    source = '{% load i18n %}{% translate _("original") ' + options + " %}"
    outputs = [
        kind(libraries=LIBRARIES).from_string(source).render(Context())
        for kind in [DjangoEngine, Engine]
    ]
    assert outputs == ["translated", "translated"]
