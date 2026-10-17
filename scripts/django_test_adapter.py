"""Route upstream Django test entry points through Rusty's parser and renderer."""

import django.template
import django.template.backends.django
import django.template.engine
from django.template.base import Template as DjangoTemplate

import django_rusty_templates


class Template(DjangoTemplate):
    """Keep Django's constructor/context protocol, but compile and render Rusty.

    The base Template must remain untouched: Rusty's interoperability layer uses
    it to compile opaque Python nodes. Only the public test entry point changes.
    The nodelist is Rusty's inspection tree, never the renderer for this wrapper.
    """

    def compile_nodelist(self):
        self._rusty_template = self.engine._from_string(
            self.source, self.origin.name, self.name
        )
        return self._rusty_template.nodelist

    def _render(self, context):
        return self._rusty_template._render(context)


def install():
    original_default = django.template.Engine.get_default
    django_rusty_templates.Engine.get_default = original_default
    django.template.Engine = django.template.engine.Engine = (
        django_rusty_templates.Engine
    )
    django.template.backends.django.DjangoTemplates = (
        django_rusty_templates.RustyTemplates
    )
    # setup_test_environment() must instrument the base Template used by
    # interoperability fallbacks, rather than this direct-constructor adapter.
    from django.test import utils  # noqa: F401

    django.template.Template = Template
