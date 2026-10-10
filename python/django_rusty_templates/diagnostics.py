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


class ResolvedArgument:
    """Expose an already resolved native argument to Django's URL node."""

    def __init__(self, value):
        self.value = value

    def resolve(self, context):
        return self.value


def render_url(view_name, urlconf, args, kwargs, current_app):
    from django.template import Context
    from django.template.base import NodeList
    from django.template.defaulttags import URLNode

    # Use Django's normal URL-node render hooks so its resolver exceptions keep
    # the same Python traceback context as other Django template URL calls.
    context = Context(autoescape=False)
    context.request = SimpleNamespace(current_app=current_app)
    context.template = SimpleNamespace(engine=SimpleNamespace(debug=False))
    node = URLNode(
        ResolvedArgument(view_name),
        [ResolvedArgument(value) for value in args or ()],
        {name: ResolvedArgument(value) for name, value in (kwargs or {}).items()},
        None,
    )
    return "".join([NodeList([node]).render(context)])
