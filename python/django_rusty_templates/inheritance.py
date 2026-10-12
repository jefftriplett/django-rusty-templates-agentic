"""Compatibility bridge for inheritance chains containing Python templates.

Supported inheritance is managed in Rust and never calls this module. At an
unsupported parent, export compiled overrides to Django's fallback renderer.
"""

from django.template.base import Node, NodeList
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

    def render_annotated(self, context):
        # Rust owns the body source and annotates its errors. This opaque bridge
        # has no Python token; applying Django's node hook would mask errors.
        return self.render(context)


def make_block(name, template):
    return BlockNode(name, NodeList([CompiledBody(template)]))


def render_fallback_parent(parent, blocks, context, root, history):
    """Bridge a native override stack only when the parent needs Python."""
    from contextlib import nullcontext
    from django.template.base import TextNode
    from .django_rusty_templates import Template

    if isinstance(parent, Template):
        parent = parent._django_template
    binding = context.bind_template(root) if context.template is None else nullcontext()
    with binding, context.render_context.push_state(root):
        context.render_context["extends_context"] = list(history)
        block_context = BlockContext()
        for name, stack in blocks.items():
            block_context.blocks[name] = list(stack)
        context.render_context[BLOCK_CONTEXT_KEY] = block_context
        for node in parent.nodelist:
            if not isinstance(node, TextNode):
                if not isinstance(node, ExtendsNode):
                    block_context.add_blocks(
                        {
                            block.name: block
                            for block in parent.nodelist.get_nodes_by_type(BlockNode)
                        }
                    )
                break
        with context.render_context.push_state(parent, isolated_context=False):
            return parent._render(context)
