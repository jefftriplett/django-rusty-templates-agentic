from django import VERSION, template


register = template.Library()


def missing_context(request): ...


def missing_context_block(content): ...


def missing_content_block_with_context(context): ...


def missing_content_block(context): ...


@register.simple_tag(takes_context=True)
def request_path(context):
    global smuggled_context
    smuggled_context = context
    return context.request.path


# Newer Django validates these signatures immediately at registration.
if VERSION < (6, 1):
    register.simple_tag(takes_context=True)(missing_context)
    register.simple_block_tag(takes_context=True)(missing_context_block)
    register.simple_block_tag(
        takes_context=True, end_name="end_missing_content_block_with_context"
    )(missing_content_block_with_context)
    register.simple_block_tag(takes_context=False)(missing_content_block)
