"""Django 6.1's CSP tag, exercised directly through the Rusty backend."""

import re
from types import SimpleNamespace

from django import VERSION
from django.forms import Media
from django.template import Context, Engine as DjangoEngine, TemplateSyntaxError
from django.utils.safestring import mark_safe
import pytest

from django_rusty_templates import Engine, RustyTemplates


@pytest.mark.skipif(VERSION < (6, 1), reason="CSP template tag requires Django 6.1")
@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize(
    "values",
    [
        {},
        {"csp_nonce": None},
        {"csp_nonce": "abc123"},
        {"csp_nonce": ""},
        {"csp_nonce": False},
        {"csp_nonce": 0},
        {"csp_nonce": '<script>"&'},
        {"csp_nonce": mark_safe("already&amp;escaped")},
    ],
)
@pytest.mark.parametrize(
    "source",
    [
        "<script {% csp_nonce_attr %}></script>",
        "{% csp_nonce_attr as attr %}<script {{ attr }}></script>",
        "{% autoescape off %}{% csp_nonce_attr %}{% endautoescape %}",
        "{% for item in items %}{% csp_nonce_attr %}{% endfor %}",
    ],
)
def test_nonce_attribute(debug, values, source):
    values = {**values, "items": [1, 2]}
    backend = RustyTemplates(
        {"NAME": "rusty", "DIRS": [], "APP_DIRS": False, "OPTIONS": {"debug": debug}}
    )
    template = backend.from_string(source)
    assert template._is_native
    expected = DjangoEngine(debug=debug).from_string(source).render(Context(values))
    assert template.render(values) == expected
    assert template.render(Context(values)) == expected


@pytest.mark.skipif(VERSION < (6, 1), reason="CSP template tag requires Django 6.1")
@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("nonce", [None, "abc123", '<script>"&'])
@pytest.mark.parametrize(
    "source",
    [
        "{% csp_nonce_attr media %}",
        "{% csp_nonce_attr media=media %}",
        "{% csp_nonce_attr media.js %}",
        "{% csp_nonce_attr media as assets %}{{ assets }}",
    ],
)
def test_nonce_media(debug, nonce, source):
    values = {
        "csp_nonce": nonce,
        "media": Media(css={"all": ["/style.css"]}, js=["/script.js"]),
    }
    template = Engine(debug=debug).from_string(source)
    assert template._is_native
    expected = DjangoEngine(debug=debug).from_string(source).render(Context(values))
    assert template.render(Context(values)) == expected


@pytest.mark.skipif(VERSION < (6, 1), reason="CSP template tag requires Django 6.1")
@pytest.mark.parametrize(
    "source",
    [
        "{% csp_nonce_attr first second %}",
        "{% csp_nonce_attr unknown=value %}",
        "{% csp_nonce_attr media=first media=second %}",
    ],
)
def test_invalid_arguments_match_django(source):
    with pytest.raises(TemplateSyntaxError) as reference:
        DjangoEngine().from_string(source)
    with pytest.raises(TemplateSyntaxError) as rusty:
        Engine().from_string(source)
    assert str(rusty.value) == str(reference.value)


@pytest.mark.skipif(VERSION >= (6, 1), reason="CSP tag exists on Django 6.1+")
def test_nonce_tag_is_unavailable_on_older_django():
    with pytest.raises(TemplateSyntaxError, match="Invalid block tag"):
        Engine().from_string("{% csp_nonce_attr %}")


@pytest.mark.skipif(VERSION < (6, 1), reason="CSP template tag requires Django 6.1")
@pytest.mark.parametrize("debug", [False, True])
@pytest.mark.parametrize("request_context", [False, True])
@pytest.mark.parametrize("policy_mode", ["enforced", "report_only", "both"])
@pytest.mark.parametrize(
    "source",
    [
        '<script nonce="{{ csp_nonce }}"></script><style nonce="{{ csp_nonce }}"></style>',
        '<script src="/script.js" {% csp_nonce_attr %}></script>'
        '<link rel="stylesheet" href="/style.css" {% csp_nonce_attr %}>',
        "{% csp_nonce_attr form.media %}",
    ],
)
def test_documented_nonce_flow(settings, debug, request_context, policy_mode, source):
    from django.http import HttpResponse
    from django.middleware.csp import ContentSecurityPolicyMiddleware
    from django.template import RequestContext
    from django.test import RequestFactory
    from django.utils.csp import CSP
    from django.utils.functional import empty

    policy = {"script-src": [CSP.SELF, CSP.NONCE], "style-src": [CSP.SELF, CSP.NONCE]}
    settings.SECURE_CSP = policy if policy_mode != "report_only" else None
    settings.SECURE_CSP_REPORT_ONLY = policy if policy_mode != "enforced" else None
    backend = RustyTemplates(
        {
            "NAME": "rusty",
            "DIRS": [],
            "APP_DIRS": False,
            "OPTIONS": {
                "debug": debug,
                "context_processors": ["django.template.context_processors.csp"],
            },
        }
    )
    template = backend.from_string(source)
    assert template._is_native

    def view(request):
        assert request._csp_nonce._wrapped is empty
        values = {
            "form": SimpleNamespace(
                media=Media(css={"all": ["/style.css"]}, js=["/script.js"])
            )
        }
        if request_context:
            rendered = template.render(RequestContext(request, values))
        else:
            rendered = template.render(values, request)
        return HttpResponse(rendered)

    handler = ContentSecurityPolicyMiddleware(view)
    nonces = []
    for _ in range(2):
        response = handler(RequestFactory().get("/"))
        attributes = re.findall(r'nonce="([^"]+)"', response.text)
        assert len(attributes) == 2
        nonce = attributes[0]
        assert attributes == [nonce, nonce]
        for header, enabled in [
            (CSP.HEADER_ENFORCE, policy_mode != "report_only"),
            (CSP.HEADER_REPORT_ONLY, policy_mode != "enforced"),
        ]:
            if enabled:
                assert response[header].count(f"'nonce-{nonce}'") == 2
            else:
                assert header not in response
        nonces.append(nonce)
    assert nonces[0] != nonces[1]


@pytest.mark.skipif(VERSION < (6, 0), reason="CSP middleware requires Django 6.0")
@pytest.mark.parametrize("debug", [False, True])
def test_unused_nonce_stays_lazy(settings, debug):
    from django.http import HttpResponse
    from django.middleware.csp import ContentSecurityPolicyMiddleware
    from django.test import RequestFactory
    from django.utils.csp import CSP
    from django.utils.functional import empty

    settings.SECURE_CSP = {"script-src": [CSP.SELF, CSP.NONCE]}
    settings.SECURE_CSP_REPORT_ONLY = None
    backend = RustyTemplates(
        {
            "NAME": "rusty",
            "DIRS": [],
            "APP_DIRS": False,
            "OPTIONS": {
                "debug": debug,
                "context_processors": ["django.template.context_processors.csp"],
            },
        }
    )

    def view(request):
        rendered = backend.from_string("<p>No nonce used</p>").render({}, request)
        assert request._csp_nonce._wrapped is empty
        return HttpResponse(rendered)

    response = ContentSecurityPolicyMiddleware(view)(RequestFactory().get("/"))
    assert response[CSP.HEADER_ENFORCE] == "script-src 'self'"
