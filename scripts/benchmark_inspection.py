"""Measure explicit inspection separately from ordinary native rendering."""

import argparse
import json
import statistics
import timeit
from pathlib import Path

from django.conf import settings
from django.template import Context, Engine as DjangoEngine
from django.template.base import VariableNode

from django_rusty_templates import Engine


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--number", type=int, default=100)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--json-output", type=Path)
    args = parser.parse_args()
    if min(args.number, args.repeat) <= 0:
        parser.error("number and repeat must be positive")
    if not settings.configured:
        settings.configure(USE_I18N=False)
    results = {}
    for debug in (False, True):
        for name, source in {
            "flat": "text {{ value|default:'empty' }}" * 20,
            "blocks": "{% if value %}{{ value }}{% endif %}" * 20,
        }.items():
            for engine_name, engine_class in (
                ("rusty", Engine),
                ("django", DjangoEngine),
            ):
                engine = engine_class(debug=debug)
                context = Context({"value": "<&>"})

                def compile_render():
                    return engine.from_string(source).render(context)

                def compile_inspect_render():
                    template = engine.from_string(source)
                    assert len(template.nodelist.get_nodes_by_type(VariableNode)) == 20
                    return template.render(context)

                template = engine.from_string(source)
                template.nodelist.get_nodes_by_type(VariableNode)
                assert compile_render() == compile_inspect_render()
                phases = {
                    "compile_render": compile_render,
                    "compile_inspect_render": compile_inspect_render,
                    "warm_render_after_inspection": lambda: template.render(context),
                }
                key = f"debug={debug}/{name}/{engine_name}"
                results[key] = {
                    phase: statistics.median(
                        timeit.repeat(function, number=args.number, repeat=args.repeat)
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
