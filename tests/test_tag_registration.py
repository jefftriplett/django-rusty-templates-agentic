import pytest
from django import VERSION
from django.template import Library, TemplateSyntaxError

from tests.templatetags import invalid_tags


@pytest.mark.skipif(
    VERSION < (6, 1), reason="Registration-time validation starts in Django 6.1"
)
@pytest.mark.parametrize(
    "kind,function,options,message",
    [
        (
            "simple_tag",
            invalid_tags.missing_context,
            {"takes_context": True},
            "first argument of 'context'",
        ),
        (
            "simple_block_tag",
            invalid_tags.missing_context_block,
            {"takes_context": True},
            "second argument of 'content'",
        ),
        (
            "simple_block_tag",
            invalid_tags.missing_content_block_with_context,
            {"takes_context": True},
            "second argument of 'content'",
        ),
        (
            "simple_block_tag",
            invalid_tags.missing_content_block,
            {},
            "first argument of 'content'",
        ),
    ],
)
def test_invalid_tag_signature_rejected_at_registration(
    kind, function, options, message
):
    library = Library()
    with pytest.raises(TemplateSyntaxError, match=message):
        getattr(library, kind)(**options)(function)
    assert function.__name__ not in library.tags
