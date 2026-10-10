"""Django-compatible diagnostics for templates rejected by the native parser."""

from django.template.base import DebugLexer, Lexer, Origin, Parser


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
    parser.parse()
