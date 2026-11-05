import shutil
import subprocess

import pytest
from django.utils.translation import override
from django.utils.translation.reloader import translation_file_changed

from django_rusty_templates import Engine

from .utils import render


def test_translate_default():
    expected = "Welcome\n\nGoodbye\n\n"

    assert render("translation.txt", {}, using="django") == expected
    assert render("translation.txt", {}, using="rusty") == expected


def test_translate_missing():
    expected = "Welcome\n\nGoodbye\n\n"

    with override("fr"):  # Deliberately missing translation
        assert render("translation.txt", {}, using="django") == expected
        assert render("translation.txt", {}, using="rusty") == expected


def test_translate_valid():
    expected = "Willkommen\n\nAuf Wiedersehen\n\n"
    with override("de"):
        assert render("translation.txt", {}, using="django") == expected
        assert render("translation.txt", {}, using="rusty") == expected


ENGINE = Engine(libraries={"i18n": "django.templatetags.i18n"})


def translated_page():
    return ENGINE.from_string(
        "{% load i18n %}"
        "{% translate 'Welcome' %}|{% blocktranslate %}Welcome{% endblocktranslate %}"
    )


def test_translate_follows_language_across_renders():
    template = translated_page()
    loop = ENGINE.from_string(
        "{% load i18n %}{% for code in codes %}{% language code %}"
        "{% translate 'Welcome' %},{% endlanguage %}{% endfor %}"
    )

    for code, expected in [
        ("de", "Willkommen|Willkommen"),
        ("en", "Welcome|Welcome"),
        ("de", "Willkommen|Willkommen"),
    ]:
        with override(code):
            assert template.render({}) == expected
    assert (
        loop.render({"codes": ["de", "en", "de"]}) == "Willkommen,Welcome,Willkommen,"
    )


@pytest.mark.skipif(shutil.which("msgfmt") is None, reason="needs gettext")
def test_translate_sees_reloaded_catalogs(settings, tmp_path):
    # runserver reloads catalogs in-process when a .mo file changes, while
    # cached templates stay loaded.
    messages = tmp_path / "de" / "LC_MESSAGES"
    messages.mkdir(parents=True)

    def compile(translation):
        po = messages / "django.po"
        po.write_text(
            'msgid ""\nmsgstr "Content-Type: text/plain; charset=UTF-8\\n"\n\n'
            f'msgid "Welcome"\nmsgstr "{translation}"\n'
        )
        subprocess.run(["msgfmt", "-o", messages / "django.mo", po], check=True)
        translation_file_changed(None, messages / "django.mo")

    compile("Hallo")
    settings.LOCALE_PATHS = [str(tmp_path)]
    template = translated_page()
    with override("de"):
        assert template.render({}) == "Hallo|Hallo"
    compile("Servus")
    with override("de"):
        assert template.render({}) == "Servus|Servus"
