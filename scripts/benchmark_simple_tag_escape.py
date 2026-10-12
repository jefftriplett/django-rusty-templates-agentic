"""Compare repeated simple tags through loaded and from-string templates."""

import argparse
import json
import statistics
import sys
import timeit
from pathlib import Path
from types import ModuleType

from django.conf import settings
from django.template import Context, Engine as DjangoEngine, Library

from django_rusty_templates import Engine


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=200)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--json-output", type=Path)
    args = parser.parse_args()
    if min(args.number, args.repeat) <= 0:
        parser.error("number and repeat must be positive")
    if not settings.configured:
        settings.configure(USE_I18N=False)
    module = ModuleType("escape_benchmark_tags")
    module.register = Library()
    module.register.simple_tag(lambda value: value, name="echo")
    sys.modules[module.__name__] = module
    source = "{% load escape_benchmark_tags %}" + "{% echo value %}" * 20
    results = {}
    for debug in (False, True):
        for cached in (False, True):
            loaders = [("django.template.loaders.locmem.Loader", {"page.html": source})]
            if cached:
                loaders = [("django.template.loaders.cached.Loader", loaders)]
            for loaded in (False, True):
                for size, value in (("small", "<&>"), ("large", "<&>" * 1366)):
                    for name, engine_class in (
                        ("rusty", Engine),
                        ("django", DjangoEngine),
                    ):
                        engine = engine_class(
                            debug=debug,
                            loaders=loaders,
                            libraries={module.__name__: module.__name__},
                        )
                        context = Context({"value": value})

                        def compile_template():
                            return (
                                engine.get_template("page.html")
                                if loaded
                                else engine.from_string(source)
                            )

                        template = compile_template()
                        expected = (
                            "&lt;&amp;&gt;" * (1 if size == "small" else 1366) * 20
                        )
                        assert template.render(context) == expected
                        phases = {
                            "warm_render": lambda: template.render(context),
                            "load_render": lambda: compile_template().render(context),
                        }
                        key = f"debug={debug}/cached={cached}/loaded={loaded}/{size}/{name}"
                        results[key] = {
                            phase: statistics.median(
                                timeit.repeat(
                                    function, number=args.number, repeat=args.repeat
                                )
                            )
                            / args.number
                            for phase, function in phases.items()
                        }
    output = json.dumps(results, indent=2)
    print(output)
    if args.json_output:
        args.json_output.write_text(output + "\n")


if __name__ == "__main__":
    main()
