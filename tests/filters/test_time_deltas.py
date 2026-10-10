import datetime

import pytest
from django.template import Context, Engine as DjangoEngine
from django.utils import translation

from django_rusty_templates import Engine


@pytest.mark.parametrize("name", ["timesince", "timeuntil"])
@pytest.mark.parametrize(
    "value",
    [
        None,
        "",
        "invalid",
        3,
        datetime.date(2024, 1, 1),
        datetime.datetime(2024, 1, 1),
        datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone.utc),
    ],
)
@pytest.mark.parametrize("language", ["en", "de"])
def test_date_comparison(name, value, language):
    context = {"value": value, "reference": datetime.datetime(2024, 1, 3, 2, 30)}
    source = "{{ value|" + name + ":reference }}"
    with translation.override(language):
        if value in ("invalid", 3):
            for engine in (DjangoEngine(), Engine()):
                with pytest.raises(AttributeError):
                    engine.from_string(source).render(Context(context))
        else:
            expected = DjangoEngine().from_string(source).render(Context(context))
            assert Engine().from_string(source).render(Context(context)) == expected


@pytest.mark.parametrize("name", ["timesince", "timeuntil"])
def test_missing_date(name):
    source = "{{ missing|" + name + " }}"
    assert Engine().from_string(source).render({}) == ""
