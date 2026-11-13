"""`is` on ints, errors `length` and `firstof` used to swallow, and missing
attributes that used to call `str()` on the object (#412)."""

import pytest
from django.template import engines

HUGE = 10**30

CONTEXT = {
    "i": 5,
    "big": 1000,
    "big2": int("1000"),
    "huge": HUGE,
    "huge2": HUGE + 1 - 1,
    "neg": -5,
    "m6": -6,
    "c256": 256,
    "c257": 257,
    "one": 1,
    "t": True,
    "n": None,
    "f": 1.5,
    "fl1": 1.0,
    "s": "a",
    "l": [1, 2, 3],
    "hl": [HUGE],
    "ll": list(range(300)),
}

OPERANDS = [
    "i",
    "5",
    "big",
    "big2",
    "1000",
    "huge",
    "huge2",
    "1000000000000000000000000000000",
    "hl.0",
    "neg",
    "-5",
    "m6",
    "-6",
    "c256",
    "256",
    "c257",
    "257",
    "one",
    "1",
    "t",
    "True",
    "n",
    "None",
    "missing",
    "f",
    "1.5",
    "fl1",
    "1.0",
    "s",
    "'a'",
    "l|length",
    "3",
    "ll|length",
    "300",
    "l.0",
    "l|first",
    "i|add:1",
    "6",
]


@pytest.mark.parametrize("operator", ["is", "is not"])
@pytest.mark.parametrize("left", OPERANDS)
def test_is_agrees_with_django(left, operator):
    # Which ints Python shares depends on its version, so Django is the
    # oracle here, not a fixed answer.
    results = {}
    for name in ("django", "rusty"):
        source = "".join(
            f"{{% if {left} {operator} {right} %}}y{{% else %}}n{{% endif %}}"
            for right in OPERANDS
        )
        results[name] = engines[name].from_string(source).render(dict(CONTEXT))
    assert results["rusty"] == results["django"]


def test_is_holds_for_a_small_int(assert_render):
    # Every CPython shares the ints from -5 to 256.
    assert_render(
        template=(
            "{% if i is 5 %}a{% endif %}{% if i is not 5 %}b{% endif %}"
            "{% if l|length is 3 %}c{% endif %}{% if 1 is 1 %}d{% endif %}"
            "{% if i is 6 %}e{% endif %}{% if huge is huge2 %}f{% endif %}"
            "{% if huge is huge %}g{% endif %}"
        ),
        context=dict(CONTEXT),
        expected="acdg",
    )


def test_is_holds_for_loop_counters(assert_render):
    assert_render(
        template=(
            "{% for x in l %}{% if forloop.counter is 1 %}a{% endif %}"
            "{% if forloop.counter0 is 0 %}b{% endif %}"
            "{% if forloop.revcounter is 3 %}c{% endif %}"
            "{% if forloop.revcounter0 is not 0 %}d{% endif %}{% endfor %}"
        ),
        context=dict(CONTEXT),
        expected="abcdd",
    )


class Boom(RuntimeError):
    pass


def raising(error):
    class Raises:
        def __len__(self):
            raise error("boom")

    return Raises()


@pytest.mark.parametrize(
    "template",
    [
        "{{ v|length }}",
        "{% if v|length %}y{% endif %}",
        "{% firstof v 'x' %}",
        "{% firstof missing v 'x' %}",
        "{% firstof v 'x' as out %}{{ out }}",
    ],
)
@pytest.mark.parametrize("error", [Boom, KeyError, OverflowError])
def test_length_and_firstof_let_errors_escape(template_engine, template, error):
    template = template_engine.from_string(template)
    with pytest.raises(error):
        template.render({"v": raising(error)})


@pytest.mark.parametrize("error", [TypeError, ValueError])
def test_length_is_zero_for_type_and_value_errors(assert_render, error):
    assert_render(
        template="{{ v|length }}{% if v|length %}y{% else %}n{% endif %}",
        context={"v": raising(error)},
        expected="0n",
    )


class Counted:
    def __init__(self, calls):
        self.calls = calls

    def __str__(self):
        self.calls.append(1)
        return "counted"


class Unprintable:
    def __str__(self):
        raise Boom("no text")


@pytest.mark.parametrize(
    ("template", "expected"),
    [
        ("[{{ row.missing }}]", "[]"),
        ("[{{ row.missing.deeper }}]", "[]"),
        ("{{ row.missing|default:'-' }}", "-"),
        ("{% if row.missing %}y{% else %}n{% endif %}", "n"),
        ("{% if row.missing == 1 %}y{% else %}n{% endif %}", "n"),
        ("{% firstof row.missing 'x' %}", "x"),
        ("{% with y=row.missing %}[{{ y }}]{% endwith %}", "[]"),
        ("{% for x in row.missing %}{{ x }}{% empty %}none{% endfor %}", "none"),
    ],
)
def test_a_missing_attribute_doesnt_print_the_object(
    template_engine, template, expected
):
    calls = []
    template = template_engine.from_string(template)
    assert template.render({"row": Counted(calls)}) == expected
    assert calls == []
    assert template.render({"row": Unprintable()}) == expected


def test_a_reported_missing_attribute_names_the_object():
    template = engines["rusty"].from_string("{{ s|add:row.missing }}")
    with pytest.raises(Exception) as info:
        template.render({"s": "a", "row": {"key": "value"}})
    assert "Failed lookup for key [missing] in {'key': 'value'}" in str(info.value)
