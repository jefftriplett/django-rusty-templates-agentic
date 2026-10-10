import os

import pytest
from django.template import Engine as DjangoEngine, TemplateDoesNotExist
from django.test import override_settings

from django_rusty_templates import Engine


@pytest.mark.parametrize("engine_cls", [Engine, DjangoEngine])
def test_directory_read_error(engine_cls, tmp_path):
    (tmp_path / "directory").mkdir()
    engine = engine_cls(
        dirs=[tmp_path], loaders=["django.template.loaders.filesystem.Loader"]
    )
    with pytest.raises(PermissionError if os.name == "nt" else IsADirectoryError):
        engine.get_template("directory")
    with pytest.raises(TemplateDoesNotExist):
        engine.get_template("missing")


@pytest.mark.parametrize("engine_cls", [Engine, DjangoEngine])
def test_app_loader_reflects_installed_apps(engine_cls):
    engine = engine_cls(loaders=["django.template.loaders.app_directories.Loader"])
    with override_settings(INSTALLED_APPS=["tests"]):
        assert engine.get_template("basic.txt")
    with override_settings(INSTALLED_APPS=[]):
        with pytest.raises(TemplateDoesNotExist):
            engine.get_template("basic.txt")
