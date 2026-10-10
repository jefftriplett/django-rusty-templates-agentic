from django.conf import settings
from django.template import Engine as DjangoEngine
from django.template import TemplateDoesNotExist
from django.template.backends.base import BaseEngine
from django.template.backends.django import Template as BackendTemplate
from django.template.backends.django import DjangoTemplates, get_installed_libraries

from .django_rusty_templates import Engine, Template

__all__ = ["RustyTemplates", "Template"]

# Preserve Django's implementation when conformance runners replace Engine.
_django_get_template = DjangoEngine.get_template


class RustyTemplates(DjangoTemplates):
    app_dirname = "templates"

    def __init__(self, params):
        params = params.copy()
        options = params.pop("OPTIONS").copy()
        options.setdefault("autoescape", True)
        options.setdefault("debug", settings.DEBUG)
        options.setdefault("file_charset", "utf-8")
        libraries = options.get("libraries", {})
        options["libraries"] = self.get_templatetag_libraries(libraries)
        BaseEngine.__init__(self, params)
        self.engine = Engine(self.dirs, self.app_dirs, **options)

    def from_string(self, template_code):
        return self.engine.from_string(template_code)

    def get_template(self, template_name):
        try:
            template = self.engine.get_template(template_name)
            if isinstance(template, Template):
                return template
            return BackendTemplate(template, self)
        except TemplateDoesNotExist as error:
            error.backend = self
            raise

    def get_templatetag_libraries(self, custom_libraries):
        """
        Return a collation of template tag libraries from installed
        applications and the supplied custom_libraries argument.
        """
        libraries = get_installed_libraries()
        libraries.update(custom_libraries)
        return libraries
