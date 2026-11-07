"""Pytest plugin that breaks the Rusty engine on purpose.

A test that still passes while Rusty is broken isn't testing Rusty.
`scripts/check_broken_engine.py` loads this plugin and compares the tests
that survive against an allowlist.

BROKEN_ENGINE=output  Rusty renders run as usual, then return junk.
BROKEN_ENGINE=raise   every Rusty compile or render raises BrokenEngine.

Rusty's classes are Rust types and can't be patched, so the plugin swaps
`django_rusty_templates.Engine`, `PortableEngine` and the engine behind
`RustyTemplates` for proxies. It must load before test modules import them.
"""

import json
import os

import django.template.base
import pytest

import django_rusty_templates
from django_rusty_templates import RustyTemplates

MODE = os.environ["BROKEN_ENGINE"]
JUNK = "\x00broken engine\x00"
RENDER_METHODS = {"render", "_render", "_render_fragment"}
COMPILE_METHODS = {"from_string", "get_template", "select_template", "_from_string"}
# Engine methods that render straight to a string, and native-only inspection.
ENGINE_RENDER_METHODS = {"render_to_string", "render_json"}
INSPECT_METHODS = {"explain_template"}

counts = {}
results = {}


def reset():
    counts.update(rusty=0, rendered=0, django=0)


reset()


class BrokenEngine(BaseException):
    """Not an Exception, so tests can't catch it by accident."""


class TemplateProxy:
    def __init__(self, template):
        object.__setattr__(self, "_template", template)

    # Rusty's objects copy to themselves; forwarding these would hand the
    # copy the unwrapped object and hide it from the check.
    def __copy__(self):
        return self

    def __deepcopy__(self, memo):
        return self

    def __getattr__(self, name):
        attr = getattr(self._template, name)
        if name not in RENDER_METHODS:
            return attr

        def render(*args, **kwargs):
            counts["rusty"] += 1
            if MODE == "raise":
                raise BrokenEngine(name)
            attr(*args, **kwargs)
            # Count only renders that succeeded: error tests still check Rusty.
            counts["rendered"] += 1
            return JUNK

        return render

    def __setattr__(self, name, value):
        setattr(self._template, name, value)


class EngineProxy:
    def __init__(self, engine):
        object.__setattr__(self, "_engine", engine)

    # Rusty's objects copy to themselves; forwarding these would hand the
    # copy the unwrapped object and hide it from the check.
    def __copy__(self):
        return self

    def __deepcopy__(self, memo):
        return self

    def __getattr__(self, name):
        attr = getattr(self._engine, name)
        if name in COMPILE_METHODS:

            def compile_template(*args, **kwargs):
                counts["rusty"] += 1
                if MODE == "raise":
                    raise BrokenEngine(name)
                return TemplateProxy(attr(*args, **kwargs))

            return compile_template
        if name in ENGINE_RENDER_METHODS:

            def render(*args, **kwargs):
                counts["rusty"] += 1
                if MODE == "raise":
                    raise BrokenEngine(name)
                attr(*args, **kwargs)
                counts["rendered"] += 1
                return JUNK

            return render
        if name in INSPECT_METHODS and MODE == "raise":

            def inspect(*args, **kwargs):
                counts["rusty"] += 1
                raise BrokenEngine(name)

            return inspect
        return attr

    def __setattr__(self, name, value):
        setattr(self._engine, name, value)


class _ProxyMeta(type):
    def __getattr__(cls, name):
        return getattr(cls._real, name)

    def __instancecheck__(cls, obj):
        return isinstance(obj, (EngineProxy, cls._real))


def proxy_class(real):
    def new(cls, *args, **kwargs):
        return EngineProxy(real(*args, **kwargs))

    return _ProxyMeta(real.__name__, (), {"_real": real, "__new__": new})


django_rusty_templates.Engine = proxy_class(django_rusty_templates.Engine)
django_rusty_templates.PortableEngine = proxy_class(
    django_rusty_templates.PortableEngine
)

_rusty_init = RustyTemplates.__init__


def _init(self, params):
    _rusty_init(self, params)
    self.engine = EngineProxy(self.engine)


RustyTemplates.__init__ = _init

# Count Django compiles, so Django reference variants can be told apart from
# tests that never compile a template at all.
_django_init = django.template.base.Template.__init__


def _django_template_init(self, *args, **kwargs):
    counts["django"] += 1
    return _django_init(self, *args, **kwargs)


django.template.base.Template.__init__ = _django_template_init


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_call(item):
    reset()
    outcome = yield
    results[item.nodeid] = {**counts, "passed": outcome.excinfo is None}


def pytest_sessionfinish(session):
    with open(os.environ["BROKEN_ENGINE_RESULTS"], "w") as f:
        json.dump(results, f)
