import pytest
from django.template import Context, Engine as DjangoEngine

from django_rusty_templates import Engine


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
def test_unknown_charset_is_lazy(engine_class, tmp_path):
    engine = engine_class(dirs=[tmp_path], file_charset="abc")
    assert engine.file_charset == "abc"
    assert engine.from_string("Hello").render(Context()) == "Hello"
    (tmp_path / "test.html").write_text("Hello")
    with pytest.raises(LookupError):
        engine.get_template("test.html")


@pytest.mark.parametrize("engine_class", [Engine, DjangoEngine])
def test_python_codec(engine_class, tmp_path):
    text = "Hello £"
    (tmp_path / "test.html").write_bytes(text.encode("utf-7"))
    engine = engine_class(dirs=[tmp_path], file_charset="utf-7")
    assert engine.get_template("test.html").render(Context()) == text
