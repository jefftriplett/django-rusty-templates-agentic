"""A persistent Django parser for spans consumed by opaque tag compilers.

Rust owns the outer token stream. This parser is entered only by a custom
compiler, including any nested ``parser.parse()`` calls made by that compiler.
It never parses the native prefix or trailing native nodes.
"""

import re

from django.template.base import DebugLexer, NodeList, Parser, TokenType
from django.template.defaulttags import load
from django.template import Origin


class CompilerParser(Parser):
    def __init__(self, source, engine, filename=None, template_name=None):
        self.source = source
        self.engine = engine
        self.end = 0
        self.origin = Origin(filename or "<unknown source>", template_name)
        super().__init__(
            [], engine.template_libraries, engine.template_builtins, self.origin
        )
        self._source_bytes = source.encode()
        self._tokenized = False
        self.completed_spans = []

    def load(self, source):
        # Loading a library updates the persistent parser's tags and filters;
        # it doesn't require parsing any surrounding template nodes.
        token = DebugLexer(source).tokenize()[0]
        load(self, token)

    def next_token(self):
        token = super().next_token()
        self.end = max(self.end, token._rusty_end)
        return token

    def compile(self, byte_start, compiler):
        if not self._tokenized:
            start = len(self._source_bytes[:byte_start].decode())
            suffix = self.source[start:]
            tokens = DebugLexer(suffix).tokenize()
            line_offset = self.source[:start].count("\n")
            byte_cursor, char_cursor = byte_start, 0
            for token in tokens:
                left, right = token.position
                byte_cursor += len(suffix[char_cursor:left].encode())
                token._rusty_start = byte_cursor
                byte_cursor += len(suffix[left:right].encode())
                token._rusty_end = byte_cursor
                char_cursor = right
                token.position = (left + start, right + start)
                token.lineno += line_offset
                if not self.engine.debug:
                    token.position = None
            self.tokens = list(reversed(tokens))
            self._tokenized = True
        # Rust has already consumed supported nodes between custom boundaries.
        # Keep the remaining token objects, including compiler pushback.
        while self.tokens and self.tokens[-1]._rusty_start < byte_start:
            self.tokens.pop()
        self.end = byte_start
        token = self.next_token()
        assert token.token_type == TokenType.BLOCK
        self.command_stack.append((token.contents.split()[0], token))
        try:
            node = compiler(self, token)
            self.extend_nodelist(NodeList(), node, token)
        except Exception as error:
            error = self.error(token, error)
            error._rusty_compiler_error = True
            if self.engine.debug:
                from .diagnostics import get_exception_info

                error.template_debug = get_exception_info(
                    self.source, self.origin, error, error.token
                )
            raise error
        self.command_stack.pop()
        # delete_first_token() deliberately doesn't use next_token() in Django.
        # The first still-pending token provides the exact consumed boundary.
        end = self.tokens[-1]._rusty_start if self.tokens else len(self._source_bytes)
        self.completed_spans.append((byte_start, end))
        return node, end

    def canonical_error(self, native_error):
        """Validate outer syntax without executing completed compilers again."""
        pieces = []
        cursor = 0
        for start, end in self.completed_spans:
            pieces.append(self._source_bytes[cursor:start].decode())
            pieces.append(re.sub(r"[^\n]", " ", self._source_bytes[start:end].decode()))
            cursor = end
        pieces.append(self._source_bytes[cursor:].decode())
        source = "".join(pieces)
        tokens = DebugLexer(source).tokenize()
        if not self.engine.debug:
            for token in tokens:
                token.position = None
        parser = Parser(tokens, self.libraries, [], self.origin)
        parser.tags.update(self.tags)
        parser.filters.update(self.filters)
        try:
            parser.parse()
        except Exception as error:
            error.rusty_diagnostic = str(native_error)
            error.add_note(str(native_error))
            if self.engine.debug and hasattr(error, "token"):
                from .diagnostics import get_exception_info

                error.template_debug = get_exception_info(
                    self.source, self.origin, error, error.token
                )
            return error
        return native_error


class _RenderScope:
    """The binding/push-state portion of Template.render, without processors.

    The native public render entry point has already evaluated processors.
    Keeping this scope explicit avoids evaluating them again on RequestContext.
    """

    def __init__(self, context, template, processor_values):
        self.context = context
        self.template = context.template
        self.render_template = context.render_context.template
        self.processor_index = None
        self.processor_values = None
        context.render_context.template = template
        context.render_context.push()
        if context.template is None:
            context.template = template
            context.template_name = template.name
            index = getattr(context, "_processors_index", None)
            if index is not None and processor_values is not None:
                self.processor_index = index
                self.processor_values = context.dicts[index]
                context.dicts[index] = processor_values

    def close(self):
        context = self.context
        if self.processor_index is not None:
            context.dicts[self.processor_index] = self.processor_values
        context.template = self.template
        context.render_context.template = self.render_template
        context.render_context.pop()


def begin_render(context, template, processor_values=None):
    return _RenderScope(context, template, processor_values)
