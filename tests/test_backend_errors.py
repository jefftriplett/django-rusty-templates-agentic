import pytest
from django.template import TemplateDoesNotExist, engines
from django.template.loader import get_template, select_template


@pytest.mark.parametrize("backend_name", ["rusty", "django"])
def test_missing_template_backend(backend_name):
    backend = engines[backend_name]
    with pytest.raises(TemplateDoesNotExist) as caught:
        backend.get_template("missing-backend-test.html")
    assert caught.value.backend is backend
    assert caught.value.args[0] == "missing-backend-test.html"


@pytest.mark.parametrize(
    "loader,args",
    [
        (get_template, "missing-backend-test.html"),
        (select_template, ["missing-backend-test.html", "also-missing.html"]),
    ],
)
def test_backend_in_loader_chain(loader, args):
    with pytest.raises(TemplateDoesNotExist) as caught:
        loader(args)
    assert caught.value.chain
    assert all(error.backend is not None for error in caught.value.chain)


@pytest.mark.parametrize("template_name", [123, None, "", "#", "#name"])
def test_invalid_template_name(template_name):
    backend = engines["rusty"]
    with pytest.raises(TemplateDoesNotExist) as caught:
        backend.get_template(template_name)
    assert caught.value.args[0] == template_name
    assert caught.value.backend is backend
