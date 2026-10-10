import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine


@pytest.mark.parametrize("engine_cls", [Engine, DjangoEngine])
@pytest.mark.parametrize(
    ("source", "invalid", "expected"),
    [
        ("{{ missing }}", "INVALID", "INVALID"),
        ("{{ value.missing }}", "INVALID:%s", "INVALID:value.missing"),
        ('{{ missing|default:"fallback" }}', "INVALID", "INVALID"),
        ('{{ missing|default:"fallback" }}', "", "fallback"),
        ("{% if missing %}yes{% else %}no{% endif %}", "INVALID", "no"),
        ("{% with value=missing %}{{ value }}{% endwith %}", "INVALID", "INVALID"),
        ("{{ missing }}", "<missing>", "&lt;missing&gt;"),
    ],
)
def test_invalid_string_rendering(engine_cls, source, invalid, expected):
    engine = engine_cls(string_if_invalid=invalid)
    assert engine.from_string(source).render(Context({"value": {}})) == expected
