"""Django's inheritance protocol over bodies compiled by the Rust parser.

No lexer or parser runs here. Django owns block stacks and context lifetimes;
each block body executes its compiled Rust nodes in one call.
"""

from django.template import Context, Template as DjangoTemplate
from django.template.base import Node, NodeList
from django.template.context import make_context
from django.template.loader_tags import (
    BLOCK_CONTEXT_KEY,
    BlockContext,
    BlockNode,
    ExtendsNode,
)


class CompiledBody(Node):
    def __init__(self, template):
        self.template = template

    def render(self, context):
        return self.template._render_fragment(context)


def make_block(name, template):
    return BlockNode(name, NodeList([CompiledBody(template)]))


class ParentValue:
    def __init__(self, value, token, variable):
        self.value = value
        self.token = token
        self.filters = variable
        self.var = None

    def resolve(self, context):
        return self.value


class CompiledExtends(ExtendsNode):
    def get_parent(self, context):
        from .django_rusty_templates import Template

        parent = self.parent_name.value
        underlying = getattr(parent, "template", parent)
        if isinstance(underlying, (Template, DjangoTemplate)):
            return underlying
        return super().get_parent(context)

    def render(self, context):
        from .django_rusty_templates import Template

        parent = self.get_parent(context)
        block_context = context.render_context.setdefault(
            BLOCK_CONTEXT_KEY, BlockContext()
        )
        block_context.add_blocks(self.blocks)
        if isinstance(parent, Template) and parent._is_native:
            if not parent._has_extends:
                block_context.add_blocks(parent._inheritance_blocks)
        else:
            # Unsupported custom tags keep Django's existing runtime tree.
            # Native parents never need their inspection tree for rendering.
            from django.template.base import TextNode

            for node in parent.nodelist:
                if not isinstance(node, TextNode):
                    if not isinstance(node, ExtendsNode):
                        block_context.add_blocks(
                            {
                                block.name: block
                                for block in parent.nodelist.get_nodes_by_type(
                                    BlockNode
                                )
                            }
                        )
                    break
        with context.render_context.push_state(parent, isolated_context=False):
            return parent._render(context)


def render_extends(parent, token, variable, blocks, context):
    # ExtendsNode's loader history preserves same-name recursive inheritance
    # across template directories and supports Django/custom loaders.
    node = CompiledExtends(NodeList(), ParentValue(parent, token, variable))
    node.blocks = blocks
    node.origin = context.render_context.template.origin
    return node.render(context)


def render_template(template, context=None, request=None):
    if not isinstance(context, Context):
        context = make_context(context, request, autoescape=template.engine.autoescape)
    with context.render_context.push_state(template):
        if context.template is None:
            with context.bind_template(template):
                context.template_name = template.name
                return template._render(context)
        return template._render(context)
