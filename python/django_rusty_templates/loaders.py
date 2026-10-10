"""Django loader interfaces backed by the native loader state."""

from functools import cached_property

from django.template import Origin
from django.template.loaders.base import Loader as BaseLoader
from django.template.loaders.cached import Loader as CachedLoader


class LoaderOrigin(Origin):
    def __init__(self, native, loader):
        super().__init__(native.name, native.template_name, loader)
        self._native_origin = native

    def __eq__(self, other):
        return self._native_origin == getattr(other, "_native_origin", other)

    @property
    def loader_name(self):
        return self._native_origin.loader_name


class LoaderView(BaseLoader):
    def __init__(self, native):
        self._native = native

    def __getattr__(self, name):
        return getattr(self._native, name)

    def __eq__(self, other):
        return self._native == getattr(other, "_native", other)

    @cached_property
    def loaders(self):
        return [wrap_loader(loader) for loader in self._native.loaders]

    def get_dirs(self):
        return self._native.get_dirs()

    def get_template(self, template_name, skip=None):
        if skip:
            raise NotImplementedError("Skipping template origins is not supported yet.")
        return self._native.get_template(template_name)

    def get_template_sources(self, template_name):
        if self._native.is_cached:
            for loader in self.loaders:
                yield from loader.get_template_sources(template_name)
        else:
            for origin in self._native.get_template_sources(template_name):
                yield LoaderOrigin(origin, self)

    def get_contents(self, origin):
        return self._native.get_contents(getattr(origin, "_native_origin", origin))

    def reset(self):
        super().reset()
        self._native.reset()


class CachedLoaderView(LoaderView, CachedLoader):
    pass


def wrap_loader(native):
    if native.is_cached:
        return CachedLoaderView(native)
    return LoaderView(native)
