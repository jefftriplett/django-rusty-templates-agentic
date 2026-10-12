"""Compare loaded-template compile and render costs at parser boundaries.

Use a release extension. Source fixtures are identical across engines and builds;
DEBUG and loader caching vary independently. This measures synthetic templates.
--number is a minimum batch size; short operations calibrate to at least 10ms.
Each paired round normalizes per-operation timings across the engines.
"""

import argparse
import hashlib
import json
import platform
import sys
import tempfile
import warnings
from pathlib import Path
from types import ModuleType, SimpleNamespace

from benchmark_pairs import MEASUREMENT, MINIMUM_DURATION, compare_pairs, measure_pairs

import django
from django.conf import settings
from django.template import (
    Context,
    Engine as DjangoEngine,
    Library,
    Node,
    Origin,
    Template,
)

from django_rusty_templates import Engine


def sources():
    variables = "{{ value }}" * 20
    cases = {
        "flat": variables,
        "literal_syntax": ".. 1e+5 2E-3 {# {{ object..name }} #}" + variables,
        "static": "{% load static %}" + "{% static path %}" * 20,
        "custom_late_large": "{% load marker %}{% if enabled %}"
        + "{{ value }}" * 1000
        + "{% marker %}{% endif %}",
        "cache": "{% load cache %}{% cache 0 parsing-benchmark %}"
        + variables
        + "{% endcache %}",
        "translation": "{% load i18n %}"
        + '{% translate "Hello" %}' * 10
        + "{% blocktranslate with item=value %}Hello {{ item }}{% endblocktranslate %}",
        "formatting_scopes": '{% load l10n tz i18n %}{% language "en" %}{% timezone "UTC" %}{% localtime off %}{% localize off %}'
        + variables
        + "{% endlocalize %}{% endlocaltime %}{% endtimezone %}{% endlanguage %}",
        "custom_late": "{% load marker %}" + variables + "{% marker %}" + variables,
    }
    if django.VERSION >= (6, 0):
        cases["numeric"] = "{{ 1e+5|default:1E-5 }}" * 20
        cases["partial"] = (
            "{% partialdef card inline %}"
            + variables
            + "{% endpartialdef %}{% partial card %}"
        )
    if django.VERSION >= (6, 1):
        cases["empty_lookup"] = "{{ object..name }}" * 20
    if django.VERSION >= (6, 2):
        cases["now"] = "{% now format %}" * 20
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=30)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--json-output", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--max-regression", type=float, default=0.15)
    args = parser.parse_args()
    if min(args.number, args.repeat) <= 0:
        parser.error("number and repeat must be positive")
    if not settings.configured:
        settings.configure(
            USE_I18N=False, STATIC_URL="/static/", SECRET_KEY="benchmark"
        )
    django.setup()
    module = ModuleType("rusty_parser_benchmark_tags")
    module.register = Library()
    sys.modules[module.__name__] = module

    class Marker(Node):
        def render(self, context):
            return ""

    @module.register.tag("marker")
    def marker(parser, token):
        return Marker()

    marker.rusty_isolated_compiler = True

    libraries = {
        name: "django.templatetags." + name
        for name in ["static", "cache", "i18n", "l10n", "tz"]
    }
    libraries["marker"] = module.__name__
    fixtures = sources()
    values = {
        "value": "<&>",
        "enabled": True,
        "path": "app/main.css",
        "object": {"": {"name": "ok"}},
        "format": "Y",
    }
    operations = {}
    with (
        warnings.catch_warnings(),
        tempfile.TemporaryDirectory(prefix="rusty-parser-") as directory,
    ):
        warnings.simplefilter("ignore", PendingDeprecationWarning)
        for case, source in fixtures.items():
            filename = Path(directory, case + ".html")
            filename.write_text(source)
            for debug in [False, True]:
                for cached in [False, True]:
                    loader = ("django.template.loaders.filesystem.Loader", [directory])
                    options = dict(
                        debug=debug,
                        libraries=libraries,
                        loaders=[("django.template.loaders.cached.Loader", [loader])]
                        if cached
                        else [loader],
                    )
                    expected = (
                        DjangoEngine(**options)
                        .get_template(filename.name)
                        .render(Context(values))
                    )
                    for arm, kind in [("django", DjangoEngine), ("rusty", Engine)]:
                        engine = kind(**options)
                        loaded = engine.get_template(filename.name)
                        assert loaded.render(Context(values)) == expected
                        if arm == "rusty":
                            origin = Origin(
                                str(filename),
                                template_name=filename.name,
                                loader=SimpleNamespace(
                                    _native=getattr(
                                        loaded.origin.loader,
                                        "_native",
                                        loaded.origin.loader,
                                    )
                                ),
                            )

                            def compile_template(
                                engine=engine,
                                source=source,
                                origin=origin,
                                name=filename.name,
                            ):
                                return engine._compile_loaded_template(
                                    source, origin, name
                                )
                        else:
                            origin = loaded.origin

                            def compile_template(
                                engine=engine,
                                source=source,
                                origin=origin,
                                name=filename.name,
                            ):
                                return Template(
                                    source,
                                    origin=origin,
                                    name=name,
                                    engine=engine,
                                )

                        assert compile_template().render(Context(values)) == expected

                        def render(loaded=loaded):
                            return loaded.render(Context(values))

                        def load_and_render(engine=engine, name=filename.name):
                            return engine.get_template(name).render(Context(values))

                        for phase, operation in [
                            ("compile", compile_template),
                            ("warm_render", render),
                            ("load_and_render", load_and_render),
                        ]:
                            key = f"{case}/debug={debug}/cached={cached}/{arm}/{phase}"
                            operations[key] = (operation, args.number, 1)
        results, paired_ratios = measure_pairs(operations, args.repeat)
    report = dict(
        measurement=MEASUREMENT,
        minimum_duration=MINIMUM_DURATION,
        paired_ratios=paired_ratios,
        python=platform.python_version(),
        django=django.get_version(),
        number=args.number,
        repeat=args.repeat,
        sources={
            name: hashlib.sha256(source.encode()).hexdigest()
            for name, source in fixtures.items()
        },
        microseconds=results,
    )
    if args.json_output:
        args.json_output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if args.baseline:
        base = json.loads(args.baseline.read_text())
        for field in ["python", "django", "number", "repeat", "sources"]:
            if base[field] != report[field]:
                raise SystemExit(f"Incompatible baseline: {field}")
        compare_pairs(base, report, args.max_regression)


if __name__ == "__main__":
    main()
