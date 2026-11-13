"""`title`, `striptags` and the HTML truncation filters stay native for
non-ASCII text and character references (#417). Each value renders the same
as Django, including the ones still left to Django's Python code."""

import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine

TITLES = [
    "josé garcía-lópez",
    "ÉCOLE normale",
    "o'élan and é'élan",
    "ñandú's",
    "1éa 2Éa 3a",
    "١a ١b",
    "москва и ЛЕНИНГРАД",
    "αθήνα ΚΑΙ ρώμη",
    "հայաստան",
    "istanbul İSTANBUL ı",
    "中文 mixed 文字",
    "emoji 🙂 text",
    "ǌ and ʼn",
    # Left to Python: a titlecase that isn't the uppercase, a final sigma,
    # and cased letters past Armenian.
    "straße ßeta",
    "ǆungla ǅ ǲ",
    "ὈΔΥΣΣΕΎΣ ΣΟΦΟΣ",
    "ﬁre ﬂy",
    "և",
    "ᾳδης",
    "ა ბ ǆ",
    "𐐨𐐩 deseret",
]

MARKUP = [
    "<p>Fish &amp; chips are <b>good</b> &mdash; really</p>",
    "<b>a &lt; b</b> and <i>c &gt; d</i>",
    "<p>&#38; &#x26; &#X26; &#8230; &#160;x</p>",
    "<p>&quot;quoted&quot; &apos;single&apos; &nbsp;space</p>",
    "<p>R&amp;D &copy; 2026 &euro;5 &pound;4 &deg; &times;</p>",
    "<p>a & b && c &; d</p>",
    "<p>tom &amp; jerry</p> trailing &amp; text",
    "<p>one two three four five six seven eight</p>",
    "<a href='x?a=1&b=2'>link</a> &amp; more",
    "<p>&amp;</p><p>&lt;</p>",
    # Left to Python: references without a `;`, unknown names, control
    # characters, and an `&` at the end.
    "<p>AT&T and R&D</p>",
    "<p>&nbsp no semicolon</p>",
    "<p>&notit; &unknown; &ampx;</p>",
    "<p>&#65 &#x41 &#0; &#x80; &#127; &#xD800;</p>",
    "<p>&#12ab; &# &#x &#;</p>",
    "<p>ends with &</p>",
    "<p>ends with &amp</p>",
    "<p>text</p>&",
    "<p>a &a-b; &a.b; c</p>",
]

FILTERS = [
    "striptags",
    "truncatewords_html:3",
    "truncatewords_html:50",
    "truncatechars_html:1",
    "truncatechars_html:9",
    "truncatechars_html:200",
]


def render_both(source, value):
    source = f"{{% autoescape off %}}{source}{{% endautoescape %}}"
    expected = DjangoEngine().from_string(source).render(Context({"s": value}))
    return expected, Engine().from_string(source).render(Context({"s": value}))


@pytest.mark.parametrize("value", TITLES)
def test_title_matches_django(value):
    expected, actual = render_both("{{ s|title }}", value)
    assert actual == expected


@pytest.mark.parametrize("filter_", FILTERS)
@pytest.mark.parametrize("value", MARKUP)
def test_markup_filter_matches_django(value, filter_):
    expected, actual = render_both(f"{{{{ s|{filter_} }}}}", value)
    assert actual == expected


def test_title_of_accented_names():
    _, actual = render_both("{{ s|title }}", "josé garcía-lópez")
    assert actual == "José García-López"


def test_striptags_keeps_complete_references():
    _, actual = render_both("{{ s|striptags }}", "<p>Fish &amp; <b>chips</b></p>")
    assert actual == "Fish &amp; chips"


def test_truncation_counts_a_reference_as_one_character():
    _, actual = render_both("{{ s|truncatechars_html:7 }}", "<p>a &amp; b c d e</p>")
    assert actual == "<p>a &amp; b …</p>"
