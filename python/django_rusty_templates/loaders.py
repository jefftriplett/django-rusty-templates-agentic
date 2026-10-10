"""Django loader interfaces backed by the native loader state."""

from django.template.loaders.base import Loader as BaseLoader
from django.template.loaders.cached import Loader as CachedLoader


class LoaderView(BaseLoader):
    def __init__(self, native):
        self._native = native

    def __getattr__(self, name):
        return getattr(self._native, name)

    def __eq__(self, other):
        return self._native == getattr(other, "_native", other)

    @property
    def loaders(self):
        return [wrap_loader(loader) for loader in self._native.loaders]

    def get_dirs(self):
        return self._native.get_dirs()

    def get_template(self, template_name, skip=None):
        if skip:
            raise NotImplementedError("Skipping template origins is not supported yet.")
        return self._native.get_template(template_name)

    def get_template_sources(self, template_name):
        return self._native.get_template_sources(template_name)

    def get_contents(self, origin):
        return self._native.get_contents(origin)

    def reset(self):
        super().reset()
        self._native.reset()


class CachedLoaderView(LoaderView, CachedLoader):
    pass


def wrap_loader(native):
    if native.is_cached:
        return CachedLoaderView(native)
    return LoaderView(native)
