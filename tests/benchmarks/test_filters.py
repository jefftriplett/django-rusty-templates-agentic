"""Per-filter loop cases for the optional CodSpeed regression dashboard.

Native filters set the baseline; the rest call Django's implementation and
show what porting each one to Rust could save.
"""

import datetime

import pytest
from django.template import engines

FILTERS = {
    "default": "{{ s|default:'n' }}",
    "lower": "{{ s|lower }}",
    "first": "{{ items|first }}",
    "floatformat": "{{ f|floatformat:2 }}",
    "join": "{{ items|join:', ' }}",
    "linebreaksbr": "{{ s|linebreaksbr }}",
    "pluralize": "{{ n|pluralize }}",
    "slice": "{{ items|slice:':2' }}",
    "striptags": "{{ s|striptags }}",
    "time": "{{ t|time:'H:i' }}",
    "truncatechars": "{{ s|truncatechars:10 }}",
    "truncatewords": "{{ s|truncatewords:2 }}",
    "urlencode": "{{ s|urlencode }}",
}

CONTEXT = {
    "values": range(100),
    "s": "Hello <b>wonderful</b>\nworld & friends",
    "items": ["alpha", "<beta>", "gamma"],
    "f": 3.14159,
    "n": 3,
    "t": datetime.time(13, 45),
}


@pytest.mark.parametrize("expression", FILTERS.values(), ids=FILTERS.keys())
def test_filter_loop(benchmark, template_engine, expression):
    source = "{% for i in values %}" + expression + "{% endfor %}"
    template = template_engine.from_string(source)
    expected = engines["django"].from_string(source).render(CONTEXT)
    assert template.render(CONTEXT) == expected
    benchmark(template.render, CONTEXT)
