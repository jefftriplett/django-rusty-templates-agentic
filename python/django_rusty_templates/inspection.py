"""Compatible inspection nodes; these never replace the native rendering tree."""

from django.template.base import (
    NodeList,
    Parser,
    TextNode,
    TokenType,
    VariableNode,
)

from .diagnostics import make_lexer


def build_nodelist(
    source, libraries, builtins, origin, debug, allow_multiline_tags=True
):
    tokens = make_lexer(source, debug, allow_multiline_tags).tokenize()
    parser = Parser(tokens, libraries, builtins, origin)
    # Stateful tags and library compilers require Django's complete inspection
    # grammar. Keep this compatibility parse explicit and cached by Template.
    if any(token.token_type == TokenType.BLOCK for token in tokens):
        return parser.parse()
    nodes = NodeList()
    for token in tokens:
        if token.token_type == TokenType.TEXT:
            node = TextNode(token.contents)
        elif token.token_type == TokenType.VAR:
            node = VariableNode(parser.compile_filter(token.contents))
        else:  # Comments have no inspection node in Django.
            continue
        parser.extend_nodelist(nodes, node, token)
    return nodes
