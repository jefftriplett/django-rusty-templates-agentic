import pytest


class RequiredArguments:
    def __call__(self, argument):
        return argument


class NoisyCallable:
    def __call__(self):
        raise TypeError("call body failed")


class NoisyMapping:
    def __getitem__(self, key):
        raise RuntimeError("mapping failed")


class NoisyProperty:
    @property
    def value(self):
        raise AttributeError("property failed")


class SilentError(Exception):
    silent_variable_failure = True


class SilentMapping:
    def __getitem__(self, key):
        raise SilentError("silent")


class DoNotCall:
    do_not_call_in_templates = True
    value = "kept"

    def __call__(self):
        raise AssertionError("must not call")


@pytest.mark.parametrize(
    ("source", "value", "expected"),
    [
        ("{{ value }}", RequiredArguments(), ""),
        ("{{ value.missing }}", {}, ""),
        ("{{ value.9 }}", [1], ""),
        ("{{ value.0 }}", [1], "1"),
        ("{{ value.anything }}", SilentMapping(), ""),
        ("{{ value.value }}", DoNotCall(), "kept"),
        ("{{ value.items }}", {"items": "mapping wins"}, "mapping wins"),
    ],
)
def test_variable_lookup_protocols(assert_render, source, value, expected):
    assert_render(source, {"value": value}, expected)


@pytest.mark.parametrize(
    ("source", "value", "exception", "message"),
    [
        ("{{ value }}", NoisyCallable(), TypeError, "call body failed"),
        ("{{ value.key }}", NoisyMapping(), RuntimeError, "mapping failed"),
        ("{{ value.value }}", NoisyProperty(), AttributeError, "property failed"),
    ],
)
def test_noisy_variable_errors(template_engine, source, value, exception, message):
    with pytest.raises(exception, match=message):
        template_engine.from_string(source).render({"value": value})


class OverrideDictionary(dict):
    def __getitem__(self, key):
        return f"override: {key}"


class MissingDictionary(dict):
    def __missing__(self, key):
        return f"missing: {key}"


@pytest.mark.parametrize(
    ("source", "value", "expected"),
    [
        ("{{ value.key }}", OverrideDictionary(key="stored"), "override: key"),
        ("{{ value.key }}", MissingDictionary(), "missing: key"),
        ("{{ value.0 }}", {0: "integer key"}, "integer key"),
        ("{{ value.0 }}", {"0": "string key", 0: "integer key"}, "string key"),
        ("{{ value.items|length }}", {"key": "value"}, "1"),
    ],
)
def test_dictionary_lookup_fast_path_preserves_protocols(
    assert_render, source, value, expected
):
    assert_render(source, {"value": value}, expected)
