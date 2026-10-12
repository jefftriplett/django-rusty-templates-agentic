"""Django-compatible diagnostics for templates rejected by the native parser."""

from types import SimpleNamespace

from django.template.base import DebugLexer, Lexer, Origin, Parser, Template


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


def render_url(view_name, urlconf, args, kwargs, current_app):
    from django.urls import reverse

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
        from django.template import Context
        from django.template.base import Node, NodeList

        failure = error

        class ResolverFailure(Node):
            def render(self, context):
                raise failure

        context = Context(autoescape=False)
        context.template = SimpleNamespace(engine=SimpleNamespace(debug=False))
        return "".join([NodeList([ResolverFailure()]).render(context)])
