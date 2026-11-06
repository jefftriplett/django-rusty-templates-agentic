"""Django 6.2 lets block tags span lines; earlier versions keep them as text.

Each case renders with both engines on the installed Django, so the same tests
check the 6.2 behaviour and that 5.2-6.1 output is unchanged.
See https://code.djangoproject.com/ticket/35899.
"""

import sys
import warnings
from types import ModuleType

import django
import pytest
from django.template import Context, Engine as DjangoEngine, Library, Node
from django.template.exceptions import TemplateSyntaxError

from django_rusty_templates import Engine

MULTILINE_OPTION = django.VERSION[:2] >= (6, 2)

library = Library()


class UpperNode(Node):
    def __init__(self, nodelist):
        self.nodelist = nodelist

    def render(self, context):
        return self.nodelist.render(context).upper()


@library.tag
def shout(parser, token):
    nodelist = parser.parse(("endshout",))
    parser.delete_first_token()
    return UpperNode(nodelist)


@library.simple_tag
def join_args(*args):
    return "-".join(str(arg) for arg in args)


module = ModuleType("multiline_tags_test_tags")
module.register = library
sys.modules[module.__name__] = module

CONTEXT = {"foo": "cat", "bar": True, "items": ["a", "b"], "empty": ""}
CASES = [
    # Block tags spanning lines.
    "{% if\n    foo\n    or bar\n%}a{%\n    else\n%}b{%\n    endif\n%}",
    "{% with a=3\n     b='bar'|upper\n     c=foo|upper\n%}{{ a }} {{ b }} {{ c }}"
    "{% endwith %}",
    "{% for item in items\n%}{{ item }}{%\nendfor %}",
    "{% for item in empty\n%}x{% empty\n%}none{% endfor %}",
    "{% filter\nupper %}text{% endfilter %}",
    "{% spaceless\n%}<b> </b> <i></i>{% endspaceless %}",
    "{% load i18n %}{% translate\n 'hello' %}",
    "{% firstof\n empty\n foo %}",
    "\r\n{% if\r\n bar %}crlf{% endif %}",
    # Literal braces around multiline tags.
    "data='{{% for item in items %}\n\"{{ item }}\": 1,\n{% endfor %}}'",
    "{{% if bar %}\nx{% endif %}}",
    # Variables and comments still end at a newline.
    "{{ foo\n }}",
    "{# a\n #}after",
    "{{ foo }}{# x #}{%\nif bar %}y{% endif %}",
    # Text that only looks like a tag.
    "<p>The open tag: {%</p>\n<p>The close tag: %}</p>",
    "The open tag: {%\nif foo %}",
    "{% if bar %}\n<p>{%</p>\n<p>%}</p>\n{% endif %}",
    "{%" + "a" * 150 + "\n" + "b" * 150 + "%}",
    "{%\n%}",
    "<p>{% templatetag openblock %}</p>\n<p>{% templatetag closeblock %}</p>",
    "{% if bar %}unclosed{%\nendif",
    # Named closing tags.
    "{% block nav %}header{%\n    endblock\n    nav\n%}",
    "{% block nav %}header{% endblock  nav %}",
    "{% block nav %}header{% endblock other %}",
    "{% partialdef greeting %}Hello!{%\n    endpartialdef\n    greeting\n%}"
    "{% partial greeting %}",
    "{% partialdef greeting %}Hello!{% endpartialdef  greeting %}",
    # Raw blocks.
    "{% verbatim %}content{%\nendverbatim\n%}",
    "{% verbatim example %}content{%\n    endverbatim\n    example\n%}",
    "{% verbatim a  b %}{% endverbatim a b %}{% endverbatim a  b %}",
    "{% verbatim a  b %}x{% endverbatim a  b %}",
    "{% verbatim %}\n{%\n{% endverbatim %}\n",
    "{% verbatim %}{% endverbatimx %}{% endverbatim %}",
    "{% verbatim\nx %}c{% endverbatim x %}",
    "{%verbatim%}{{ x }}{%endverbatim%}",
    "{% comment %}content{%\nendcomment\n%}after",
    "{% comment %}\n{%\n{% endcomment %}after",
    "{%comment%}{% hidden{%endcomment%}visible",
    "{% comment 'note' %}{% if %}{% endcomment %}after",
    "{% comment %}{% endcomment extra %}{% endcomment %}after",
    "{% comment %}{% endcomment x %}{% verbatim %}{% endcomment %}after",
    # Tags from a library, compiled by Django's parser.
    "{% load multiline_tags_test_tags %}{% shout\n%}{{ foo }}{%\nendshout %}",
    "{% load multiline_tags_test_tags %}{% join_args\n 1\n 2 %}",
    "{% load\n multiline_tags_test_tags %}{% join_args 3 %}",
]


def render(engine_class, source, **options):
    options["libraries"] = {module.__name__: module.__name__}
    try:
        template = engine_class(**options).from_string(source)
        return template.render(Context(dict(CONTEXT)))
    except TemplateSyntaxError:
        return TemplateSyntaxError


def engine_options(allow_multiline_tags):
    if allow_multiline_tags is None:
        return {}
    return {"allow_multiline_tags": allow_multiline_tags}


@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("source", CASES)
@pytest.mark.parametrize(
    "allow_multiline_tags",
    [None, True, False] if MULTILINE_OPTION else [None],
)
def test_multiline_tags_match_django(source, debug, allow_multiline_tags):
    options = {"debug": debug, **engine_options(allow_multiline_tags)}
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        expected = render(DjangoEngine, source, **options)
        assert render(Engine, source, **options) == expected


@pytest.mark.skipif(not MULTILINE_OPTION, reason="Django 6.2 lexes multiline tags")
@pytest.mark.parametrize(
    "source",
    [
        "{% if bar %}\n<p>{%</p>\n<p>%}</p>\n{% endif %}",
        "<p>{%</p>\n<p>%}</p>",
        "The open tag: {%\nif foo %}",
        "{%" + "a" * 150 + "\n" + "b" * 150 + "%}",
    ],
)
def test_multiline_error_messages_match_django(source):
    def message(engine_class):
        with pytest.raises(TemplateSyntaxError) as error:
            engine_class().from_string(source)
        return str(error.value)

    actual = message(Engine)
    assert actual == message(DjangoEngine)
    assert "If the '{%' and '%}' sequences were intended as text" in actual


@pytest.mark.skipif(not MULTILINE_OPTION, reason="Django 6.2 lexes multiline tags")
def test_multiline_error_debug_info_matches_django():
    source = "<p>The open tag: {%</p>\n<p>The close tag: %}</p>"

    def debug_info(engine_class):
        with pytest.raises(TemplateSyntaxError) as error:
            engine_class(debug=True).from_string(source)
        info = error.value.template_debug
        return {key: info[key] for key in ("during", "line", "start", "end", "after")}

    assert debug_info(Engine) == debug_info(DjangoEngine)


@pytest.mark.skipif(not MULTILINE_OPTION, reason="Django 6.2 added the option")
@pytest.mark.parametrize("value", [True, False])
def test_allow_multiline_tags_option_warns(value):
    from django.utils.deprecation import RemovedInDjango2029Warning

    message = (
        "The 'allow_multiline_tags' transitional template engine option is deprecated."
    )
    with pytest.warns(RemovedInDjango2029Warning, match=message) as record:
        engine = Engine(allow_multiline_tags=value)
    assert engine.allow_multiline_tags is value
    # Like Django's, the warning points at the code that built the engine.
    assert record[0].filename == __file__


@pytest.mark.skipif(not MULTILINE_OPTION, reason="Django 6.2 added the option")
def test_backend_option_disables_multiline_tags():
    from django.template import engines
    from django.test import override_settings
    from django.utils.deprecation import RemovedInDjango2029Warning

    backend = "django_rusty_templates.RustyTemplates"
    templates = [{"BACKEND": backend, "OPTIONS": {"allow_multiline_tags": False}}]
    with override_settings(TEMPLATES=templates):
        engines._engines = {}
        engines.__dict__.pop("templates", None)
        with pytest.warns(RemovedInDjango2029Warning) as record:
            engine = engines.all()[0]
        # Skips the backend's frames as well as Django's.
        assert record[0].filename == __file__
        source = "{%\ncsrf_token\n%}"
        assert engine.from_string(source).render() == source
    engines._engines = {}
    engines.__dict__.pop("templates", None)


def test_default_engine_allows_multiline_tags():
    assert Engine().allow_multiline_tags is True


@pytest.mark.skipif(MULTILINE_OPTION, reason="Django 6.2 added the option")
def test_allow_multiline_tags_rejected_before_django_6_2():
    with pytest.raises(TypeError):
        DjangoEngine(allow_multiline_tags=False)
    with pytest.raises(TypeError, match="allow_multiline_tags"):
        Engine(allow_multiline_tags=False)
