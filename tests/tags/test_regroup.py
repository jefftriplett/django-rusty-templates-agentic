import pytest
from django.template import Context, Engine as DjangoEngine, TemplateSyntaxError

from django_rusty_templates import Engine


@pytest.mark.parametrize(
    "expression,values,expected",
    [
        (
            "key",
            [{"key": "a"}, {"key": "a"}, {"key": "b"}, {"key": "a"}],
            "a:2;b:1;a:1;",
        ),
        (
            'key|join:"-"',
            [{"key": ["a", "b"]}, {"key": ["a", "b"]}, {"key": ["c"]}],
            "a-b:2;c:1;",
        ),
        ("missing", [{"key": "a"}, {"key": "b"}], "None:2;"),
        ("key", [], ""),
        ("key", None, ""),
    ],
)
def test_regroup(expression, values, expected):
    source = (
        "{% regroup values by "
        + expression
        + " as groups %}{% for key, items in groups %}{{ key }}:{{ items|length }};{% endfor %}"
    )
    for engine in (Engine(), DjangoEngine()):
        assert (
            engine.from_string(source).render(Context({"values": values})) == expected
        )


@pytest.mark.parametrize(
    "source",
    [
        "{% regroup values key as groups %}",
        "{% regroup values wrong key as groups %}",
        "{% regroup values by key wrong groups %}",
    ],
)
def test_regroup_syntax(source):
    for engine in (Engine(), DjangoEngine()):
        with pytest.raises(TemplateSyntaxError):
            engine.from_string(source)


class Country:
    def __init__(self, name):
        self.name = name

    def upper(self):
        return self.name.upper()


class City:
    def __init__(self, name, country):
        self.name = name
        self.country = country

    def __str__(self):
        return self.name


class Silent(Exception):
    silent_variable_failure = True


class Item:
    def __init__(self, value):
        self.value = value

    @property
    def broken(self):
        raise Silent

    def delete(self):
        raise AssertionError("alters_data methods must not be called")

    delete.alters_data = True

    def __str__(self):
        return str(self.value)


FRANCE, JAPAN = Country("France"), Country("Japan")
CITIES = [
    City("Paris", FRANCE),
    City("Lyon", FRANCE),
    City("Tokyo", JAPAN),
    City("Nice", FRANCE),
]


@pytest.mark.parametrize(
    ("key", "values"),
    [
        ("country.name", CITIES),
        ("country.upper", CITIES),
        ("country", CITIES),
        ("0", [["a", 1], ["a", 2], ["b", 3]]),
        ("pair.1", [{"pair": (1, "x")}, {"pair": (2, "x")}, {"pair": (3, "y")}]),
        ("missing.deeper", CITIES),
        ("broken", [Item(1), Item(2)]),
        ("delete", [Item(1), Item(2)]),
        # Lookup continues on string_if_invalid after an uncallable value.
        ("delete.upper", [Item(1), Item(2)]),
        ("value", [Item(1), Item(1.0), Item(True), Item(2)]),
    ],
)
@pytest.mark.parametrize("string_if_invalid", ["", "INVALID"])
def test_regroup_plain_keys_match_django(key, values, string_if_invalid):
    source = (
        "{% regroup values by " + key + " as groups %}"
        "{% for group in groups %}{{ group.grouper }}:"
        "{% for item in group.list %}{{ item }},{% endfor %};{% endfor %}"
    )
    outputs = [
        engine_class(string_if_invalid=string_if_invalid)
        .from_string(source)
        .render(Context({"values": values}))
        for engine_class in (Engine, DjangoEngine)
    ]
    assert outputs[0] == outputs[1]


def test_regroup_plain_key_inside_a_loop():
    source = (
        "{% for n in numbers %}{% regroup values by country.name as groups %}"
        "{{ n }}={{ groups|length }} {% endfor %}{{ groups.0.grouper }}"
    )
    outputs = [
        engine_class()
        .from_string(source)
        .render(Context({"values": CITIES, "numbers": [1, 2]}))
        for engine_class in (Engine, DjangoEngine)
    ]
    assert outputs[0] == outputs[1] == "1=3 2=3 "
