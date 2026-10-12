"""Django runtime services for stock tags whose syntax is parsed in Rust."""

from decimal import Decimal

from django.conf import settings
from django.core.cache import InvalidCacheBackendError, caches
from django.core.cache.utils import make_template_fragment_key
from django.template import TemplateSyntaxError
from django.template.base import render_value_in_context
from django.templatetags.i18n import GetLanguageInfoListNode
from django.templatetags.static import PrefixNode, StaticNode
from django.utils import timezone, translation
from django.utils.html import conditional_escape
from django.utils.safestring import SafeData, SafeString, mark_safe


def static_url(path, autoescape):
    value = StaticNode.handle_simple(path)
    return conditional_escape(value) if autoescape else value


def prefix(setting):
    return PrefixNode.handle_simple(setting)


def assigned_value(kind, value):
    if kind == "timezone":
        return timezone.get_current_timezone_name()
    if kind == "language":
        return translation.get_language()
    if kind == "language_bidi":
        return translation.get_language_bidi()
    if kind == "available_languages":
        return [(code, translation.gettext(name)) for code, name in settings.LANGUAGES]
    if kind == "language_info":
        return translation.get_language_info(value)
    node = GetLanguageInfoListNode(None, None)
    return [node.get_language_info(language) for language in value]


def override(kind, value):
    return (
        translation.override(value) if kind == "language" else timezone.override(value)
    )


def cache_entry(timeout, name, vary, cache, explicit_cache):
    if timeout is not None:
        try:
            timeout = int(timeout)
        except (ValueError, TypeError):
            raise TemplateSyntaxError(
                '"cache" tag got a non-integer timeout value: %r' % timeout
            )
    if explicit_cache:
        try:
            backend = caches[cache]
        except InvalidCacheBackendError:
            raise TemplateSyntaxError(
                "Invalid cache name specified for cache tag: %r" % cache
            )
    else:
        try:
            backend = caches["template_fragments"]
        except InvalidCacheBackendError:
            backend = caches["default"]
    key = make_template_fragment_key(name, vary)
    return backend.get(key), backend, key, timeout


def translate(value, noop, message_context, eager):
    if noop:
        return value
    safe = isinstance(value, SafeData)
    message = value.replace("%", "%%")
    if safe:
        message = mark_safe(message)
    if message_context:
        return (
            translation.pgettext(message_context, message)
            if eager
            else translation.pgettext_lazy(message_context, message)
        )
    return translation.gettext(message) if eager else translation.gettext_lazy(message)


def finish_translation(value, context):
    # Ordinary strings need escaping, but cannot need timezone conversion or
    # localization. Keep Django's full formatter for other filter results.
    if type(value) is str or type(value) is SafeString:
        output = conditional_escape(value) if context.autoescape else str(value)
    else:
        output = render_value_in_context(value, context)
    safe = isinstance(output, SafeData)
    output = output.replace("%%", "%")
    return mark_safe(output) if safe else output


def block_translation_message(
    singular, plural, count, counter_name, message_context, trimmed, tag
):
    if trimmed:
        singular = translation.trim_whitespace(singular)
        if plural is not None:
            plural = translation.trim_whitespace(plural)
    if plural:
        if not isinstance(count, (Decimal, float, int)):
            raise TemplateSyntaxError(
                "%r argument to %r tag must be a number." % (counter_name, tag)
            )
        result = (
            translation.npgettext(message_context, singular, plural, count)
            if message_context
            else translation.ngettext(singular, plural, count)
        )
    else:
        result = (
            translation.pgettext(message_context, singular)
            if message_context
            else translation.gettext(singular)
        )
    return result


def finish_block_translation(result, data, tag, nested):
    try:
        return result % data
    except (KeyError, ValueError):
        if nested:
            raise TemplateSyntaxError(
                "%r is unable to format string returned by gettext: %r using %r"
                % (tag, result, data)
            )
        return None
