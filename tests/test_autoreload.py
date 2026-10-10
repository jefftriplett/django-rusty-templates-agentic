from django.template import autoreload, engines
from django.template.backends.django import DjangoTemplates
from django.test import override_settings

from django_rusty_templates import RustyTemplates


def test_autoreload_resets_native_template_cache(tmp_path):
    path = tmp_path / "page.html"
    path.write_text("first")
    configuration = [
        {
            "BACKEND": "django_rusty_templates.RustyTemplates",
            "NAME": "rusty",
            "DIRS": [tmp_path],
        }
    ]
    with override_settings(TEMPLATES=configuration):
        backend = engines["rusty"]
        assert isinstance(backend, DjangoTemplates)
        assert isinstance(backend, RustyTemplates)
        assert tmp_path in autoreload.get_template_directories()
        assert backend.get_template("page.html").render({}) == "first"
        path.write_text("second")
        assert backend.get_template("page.html").render({}) == "first"
        assert autoreload.template_changed(None, path) is True
        assert backend.get_template("page.html").render({}) == "second"


def test_empty_template_directory_is_not_watched():
    configuration = [
        {
            "BACKEND": "django_rusty_templates.RustyTemplates",
            "NAME": "rusty",
            "DIRS": [""],
            "OPTIONS": {"loaders": ["django.template.loaders.filesystem.Loader"]},
        }
    ]
    with override_settings(TEMPLATES=configuration):
        assert autoreload.get_template_directories() == set()
