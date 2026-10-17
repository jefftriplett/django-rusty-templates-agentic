import subprocess
import sys
import textwrap

from scripts.run_django_test_suite import default_test_labels, summarize_results

import pytest
from django.template import Context, Origin
from django.template.base import Template as DjangoTemplate

from django_rusty_templates import Engine
from scripts.django_test_adapter import Template


@pytest.mark.parametrize("version", ["5.2", "6.0", "6.1", "6.2"])
def test_default_suites_include_admin_and_supported_csp_tests(version):
    labels = default_test_labels(version)
    assert labels[:3] == ["template_tests", "template_loader", "admin_views"]
    csp_labels = {
        "context_processors.tests.CSPContextProcessorTests",
        "middleware.test_csp",
        "decorators.test_csp",
        "utils_tests.test_csp",
    }
    assert csp_labels.issubset(labels) == (version != "5.2")


def test_install_routes_the_public_template_entry_point(monkeypatch):
    import django.template
    import django.template.backends.django
    import django.template.engine
    from scripts.django_test_adapter import install

    for module, name in [
        (django.template, "Engine"),
        (django.template, "Template"),
        (django.template.engine, "Engine"),
        (django.template.backends.django, "DjangoTemplates"),
    ]:
        monkeypatch.setattr(module, name, getattr(module, name))
    monkeypatch.setattr(Engine, "get_default", None, raising=False)
    install()
    template = django.template.Template("{{ value }}", engine=Engine())
    assert template._rusty_template._is_native
    assert template.render(Context({"value": "native"})) == "native"


def test_direct_template_compiles_and_renders_rusty(monkeypatch):
    def python_render_is_a_bug(*args):
        raise AssertionError("Direct Template() bypassed Rusty")

    monkeypatch.setattr(DjangoTemplate, "_render", python_render_is_a_bug)
    template = Template("{{ value }}", engine=Engine())
    assert template._rusty_template._is_native
    assert template.render(Context({"value": "<native>"})) == "&lt;native&gt;"


def test_direct_template_does_not_hide_rusty_compile_errors():
    class FailingEngine:
        debug = False

        def _from_string(self, *args):
            raise NotImplementedError("Missing native tag")

    with pytest.raises(NotImplementedError, match="Missing native tag"):
        Template("{% missing %}", engine=FailingEngine())


def test_direct_template_preserves_origin_and_name():
    template = Template(
        "{{ value }}",
        origin=Origin("/templates/direct.html", template_name="direct.html"),
        name="direct.html",
        engine=Engine(),
    )
    assert template._rusty_template.origin.name == "/templates/direct.html"
    assert template._rusty_template.name == "direct.html"


def test_bootstrap_preserves_native_and_fallback_render_signals():
    # Import order matters: Django's test instrumentation must patch the base
    # Template used by fallbacks, not the public direct-constructor adapter.
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            textwrap.dedent(
                """
                import sys
                from types import ModuleType
                from django.conf import settings
                from scripts.django_test_adapter import install

                install()
                settings.configure(
                    USE_I18N=False,
                    TEMPLATES=[{
                        "BACKEND": "django_rusty_templates.RustyTemplates",
                        "NAME": "rusty",
                        "OPTIONS": {
                            "libraries": {"custom": "conformance_signal_tags"},
                            "loaders": [("django.template.loaders.locmem.Loader", {
                                "native.html": "{{ value }}",
                                "mixed.html": "{% load custom %}{% opaque %}{{ value }}",
                            })],
                        },
                    }],
                )
                import django
                django.setup()
                from django.template import Context, Library, Template, engines
                from django.template.base import TextNode
                from django.test.signals import template_rendered
                from django.test.utils import setup_test_environment
                from django.template.base import Template as BaseTemplate
                from django.test.utils import Template as InstrumentedTemplate

                assert InstrumentedTemplate is BaseTemplate
                module = ModuleType("conformance_signal_tags")
                module.register = Library()
                module.register.tag("opaque", lambda parser, token: TextNode(""))
                sys.modules[module.__name__] = module
                seen = []
                def receiver(sender, template, **kwargs):
                    seen.append(template.name)
                template_rendered.connect(receiver, weak=False)
                setup_test_environment()
                for name in ("native.html", "mixed.html"):
                    assert engines["rusty"].get_template(name).render({"value": "ok"}) == "ok"
                direct = Template("{{ value }}")
                assert direct._rusty_template._is_native
                assert direct.render(Context({"value": "ok"})) == "ok"
                assert seen == ["native.html", "mixed.html", None], seen
                """
            ),
        ],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr


def test_conformance_summary_counts_failing_subtest_parents_once():
    results = {
        "passed": ["passed", "passed"],
        "failures": ["mixed"],
        "errors": [],
        "skipped": ["skipped"],
        "subtests": [
            {"parent": "failed", "outcome": "failures"},
            {"parent": "failed", "outcome": "failures"},
            {"parent": "mixed", "outcome": "errors"},
        ],
    }
    summary, formatted = summarize_results(results)
    assert summary == {
        "passed": 1,
        "failures": 1,
        "errors": 1,
        "skipped": 1,
        "runnable": 3,
        "total": 4,
        "pass_percentage": 100 / 3,
    }
    assert formatted.count("failed ... FAIL") == 1
    assert "mixed ... ERROR" in formatted
    assert "33.33%" in formatted


def test_conformance_summary_with_no_runnable_tests():
    summary, _ = summarize_results(
        {
            "passed": [],
            "failures": [],
            "errors": [],
            "skipped": ["skipped"],
            "subtests": [],
        }
    )
    assert summary["runnable"] == 0
    assert summary["pass_percentage"] == 0
