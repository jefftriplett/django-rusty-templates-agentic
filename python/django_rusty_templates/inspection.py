"""Compatible inspection nodes; these never replace the native rendering tree."""

from django.template.base import (
    DebugLexer,
    Lexer,
    NodeList,
    Parser,
    TextNode,
    TokenType,
    VariableNode,
)


def build_nodelist(source, libraries, builtins, origin, debug):
    tokens = (DebugLexer(source) if debug else Lexer(source)).tokenize()
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
