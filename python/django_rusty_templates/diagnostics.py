"""Django-compatible diagnostics for templates rejected by the native parser."""

import warnings
from datetime import datetime
from types import SimpleNamespace

from django.conf import settings
from django.template import Context
from django.template.base import (
    DebugLexer,
    Lexer,
    Node,
    NodeList,
    Origin,
    Parser,
    Template,
)
from django.template.defaultfilters import date
from django.urls import reverse
from django.utils import timezone
from django.utils.html import conditional_escape


def validate_syntax(source, engine, filename=None, template_name=None):
    # Only used after native parsing fails; successful templates keep the native
    # compilation path and never pay for a second parse.
    lexer = DebugLexer(source) if engine.debug else Lexer(source)
    parser = Parser(
        lexer.tokenize(),
        engine.template_libraries,
        engine.template_builtins,
        Origin(filename or "<unknown source>", template_name=template_name),
    )
    try:
        parser.parse()
    except Exception as error:
        if engine.debug:
            attach_debug(source, error, filename)
        raise


def attach_debug(source, error, filename=None):
    """Attach Django's source metadata without replacing the exception."""
    if hasattr(error, "template_debug"):
        return
    token = getattr(error, "token", None)
    if token is None or token.position is None:
        span = getattr(error, "_rusty_token_span", None)
        if span is None:
            return
        start, _ = span
        token = next(
            (
                token
                for token in DebugLexer(source).tokenize()
                if len(source[: token.position[0]].encode())
                <= start
                < len(source[: token.position[1]].encode())
            ),
            None,
        )
    if token is not None:
        template = SimpleNamespace(
            source=source, origin=Origin(filename or "<unknown source>")
        )
        error.template_debug = Template.get_exception_info(template, error, token)


def get_exception_info(source, origin, exception, token):
    """Build Django debug metadata from source without compiling a node tree."""
    template = SimpleNamespace(source=source, origin=origin)
    return Template.get_exception_info(template, exception, token)


def render_url(view_name, urlconf, args, kwargs, current_app):
    try:
        # Arguments are already resolved in Rust. Successful reversals need no
        # Django Context, node objects or render hooks.
        return "".join(
            [
                reverse(
                    view_name,
                    urlconf=urlconf,
                    args=args,
                    kwargs=kwargs,
                    current_app=current_app,
                )
            ]
        )
    except Exception as error:
        # Keep Python template render frames on the cold failure path, including
        # the original exception and resolver traceback. Do not reverse twice.
        failure = error

        class ResolverFailure(Node):
            def render(self, context):
                raise failure

        context = Context(autoescape=False)
        context.template = SimpleNamespace(engine=SimpleNamespace(debug=False))
        return "".join([NodeList([ResolverFailure()]).render(context)])


def format_now(format_string, escape):
    """Format the current time for {% now %} as Django's NowNode does."""
    tzinfo = timezone.get_current_timezone() if settings.USE_TZ else None
    formatted = date(datetime.now(tz=tzinfo), format_string)
    return conditional_escape(formatted) if escape else formatted


def warn_double_dot(source, offset, origin):
    """Match Django 6.1's variable-tag deprecation without invoking its parser."""
    # Only exists on Django 6.1+; Rust calls this only for 6.1 syntax.
    from django.utils.deprecation import RemovedInDjango2028Warning

    line = source[:offset].count("\n") + 1
    warnings.warn(
        "Support for double-dot lookups '..' which maps to a "
        "lookup of the empty string is deprecated.\n"
        f"  Template: {origin or '<unknown source>'}\n"
        f"  Line: {line}",
        RemovedInDjango2028Warning,
        stacklevel=2,
    )
