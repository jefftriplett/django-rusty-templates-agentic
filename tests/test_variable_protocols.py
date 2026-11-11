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


class LateGetItem:
    name = "attribute"


class DisabledGetItem:
    __getitem__ = None
    name = "attribute"


class ClassGetItemOnly:
    name = "attribute"

    def __class_getitem__(cls, key):
        raise AssertionError("must not subscript the class")


class GetAttrGetItem:
    """`__getattr__` answers for `__getitem__`, which subscripting ignores."""

    def __getattr__(self, name):
        if name == "__getitem__":
            return lambda key: "from getattr"
        return "attribute"


class Slots:
    __slots__ = ("name",)

    def __init__(self):
        self.name = "slot"


def late_get_item():
    class Late(LateGetItem):
        pass

    value = Late()
    Late.__getitem__ = lambda self, key: f"item {key}"
    return value


@pytest.mark.parametrize(
    ("source", "build", "expected"),
    [
        ("{{ value.name }}", late_get_item, "item name"),
        ("{{ value.name }}", DisabledGetItem, "attribute"),
        ("{{ value.name }}", ClassGetItemOnly, "attribute"),
        ("{{ value.name }}", GetAttrGetItem, "attribute"),
        ("{{ value.name }}", Slots, "slot"),
        ("{{ value.real }}{{ value.imag }}", lambda: 3, "30"),
        ("{{ value.0 }}{{ value.stop }}", lambda: range(4, 9), "49"),
        ("{{ value.1 }}", lambda: "abc", "b"),
        ("{{ value.1 }}", lambda: b"abc", "98"),
        ("{{ value.x }}{{ value.name }}", lambda: {"x": 1, "name": "key"}, "1key"),
        ("{{ value.x.x.x }}", lambda: {"x": {"x": {"x": "deep"}}}, "deep"),
    ],
)
def test_subscript_and_attribute_lookup_by_type(assert_render, source, build, expected):
    """Subscripting is tried only when the type supports it (#379)."""
    assert_render(source, {"value": build()}, expected)


def test_lookups_from_several_threads():
    """Each thread keeps its own table of interned names (#379)."""
    from concurrent.futures import ThreadPoolExecutor

    from django.template import engines

    template = engines["rusty"].from_string(
        "{% for o in rows %}{{ o.name }}{{ o.other.name }}{% endfor %}"
    )
    rows = [{"name": i, "other": Slots()} for i in range(20)]
    expected = "".join(f"{i}slot" for i in range(20))
    with ThreadPoolExecutor(max_workers=4) as pool:
        results = list(pool.map(lambda _: template.render({"rows": rows}), range(40)))
    assert results == [expected] * 40
