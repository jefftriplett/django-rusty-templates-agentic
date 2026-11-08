"""Report which installed apps' templates compile natively with Rusty.

Run it in an environment where the apps are installed:

    python scripts/survey_templates.py crispy_forms crispy_bootstrap5
    python scripts/survey_templates.py --try-isolated --json out.json \
        --setting "MIDDLEWARE=['allauth.account.middleware.AccountMiddleware']" \
        allauth allauth.account

Every template under each app's `templates` directory goes through
`engine.explain_template()`. That only reports the first blocker, and it
reports every custom tag with its own compiler, even one that opts into
`rusty_isolated_compiler`, because inspection doesn't run Python compilers.
So the survey records each blocking custom tag, blanks out that tag and its
end tag, and inspects again until the template is native or another kind of
blocker stops it.

The report counts templates by result, lists the blocking tags with the
library that defines them and the number of templates each one blocks, and
lists every tag the surveyed apps define with their own compiler: those make
any template that uses them, including a project's own, fall back.

`--try-isolated` opts each blocking tag into `rusty_isolated_compiler` (only
in this process) and compiles the blocked templates for real, to show which
would stay native if the package opted in. See docs/third-party-templates.md.
"""

import argparse
import ast
import importlib
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

BASE_APPS = [
    "django.contrib.admin",
    "django.contrib.auth",
    "django.contrib.contenttypes",
    "django.contrib.sessions",
    "django.contrib.messages",
    "django.contrib.staticfiles",
    "django.contrib.humanize",
]


def configure(apps, extra_settings):
    import django
    from django.conf import settings

    defaults = dict(
        INSTALLED_APPS=list(dict.fromkeys(BASE_APPS + apps)),
        DATABASES={
            "default": {"ENGINE": "django.db.backends.sqlite3", "NAME": ":memory:"}
        },
        TEMPLATES=[
            {
                "BACKEND": "django_rusty_templates.RustyTemplates",
                "APP_DIRS": True,
                "OPTIONS": {"context_processors": []},
            }
        ],
        STATIC_URL="/static/",
        STATIC_ROOT="static",
        ROOT_URLCONF=__name__,
        USE_TZ=True,
        SECRET_KEY="survey",
    )
    settings.configure(**(defaults | extra_settings))
    django.setup()


urlpatterns = []


def tag_compilers():
    """Map each tag name to its compilers and the libraries that register it."""
    from django.template.backends.django import get_installed_libraries

    compilers = defaultdict(list)
    for name, module_path in get_installed_libraries().items():
        try:
            module = importlib.import_module(module_path)
        except Exception:  # noqa: BLE001 - a broken library just isn't surveyed
            continue
        register = getattr(module, "register", None)
        for tag, compiler in getattr(register, "tags", {}).items():
            compilers[tag].append((name, compiler))
    return compilers


def is_generated(compiler):
    """Whether Django's `Library` generated this compiler, as Rusty checks it.

    The generated `compile_func` is wrapped with `functools.wraps`, so its
    module and name are the tag function's; its code object isn't.
    """
    code = getattr(compiler, "__code__", None)
    return (
        code is not None
        and code.co_name == "compile_func"
        and {"func", "params", "takes_context"} <= set(code.co_freevars)
    )


def own_compiler_tags(compilers, app_names):
    """Tags the surveyed apps define with their own compile function.

    Rusty compiles the compilers `simple_tag`, `inclusion_tag` and
    `simple_block_tag` generate natively.
    Any other tag in a user's template makes Django's parser compile it,
    unless the tag opts into rusty_isolated_compiler.
    """
    from django.template.backends.django import get_installed_libraries

    paths = get_installed_libraries()
    found = defaultdict(list)
    for tag, entries in compilers.items():
        for library, compiler in entries:
            module = paths[library]
            if not any(module.startswith(app + ".") for app in app_names):
                continue
            if is_generated(compiler):
                continue
            found[library].append((tag, opts_in([(library, compiler)])))
    return found


def opts_in(compilers):
    """Whether every compiler for a tag already sets rusty_isolated_compiler."""
    return bool(compilers) and all(
        getattr(compiler, "rusty_isolated_compiler", False)
        and not getattr(compiler, "rusty_requires_full_parser", False)
        for _, compiler in compilers
    )


def blank_tag(source, tag):
    """Replace every `{% tag ... %}` and `{% endtag %}` with spaces."""
    pattern = re.compile(r"{%\s*(?:end)?" + re.escape(tag) + r"\b.*?%}", re.DOTALL)
    return pattern.sub(lambda match: re.sub(r"\S", " ", match.group()), source)


def explain(engine, source, name, compilers):
    """Every blocker for one template, and its result.

    Inspection never runs a custom tag's compiler, so it reports each one as
    a blocker, even a tag that opts into rusty_isolated_compiler. The survey
    records the tag, blanks it out and inspects again.
    """
    blockers = []
    while True:
        report = engine.explain_template(source, name)
        if report["mode"] == "native":
            break
        # The token keeps its delimiters, such as "{% crispy form %}".
        token = (report["tag"] or "").removeprefix("{%").removesuffix("%}")
        tag = next(iter(token.split()), None)
        custom = report["mode"] == "hybrid" and tag in compilers
        blockers.append(
            {
                "mode": report["mode"],
                "tag": tag,
                "custom": custom,
                "opts_in": custom and opts_in(compilers[tag]),
                "reason": report["reason"],
                "line": report["line"],
                "libraries": [library for library, _ in compilers.get(tag, [])],
            }
        )
        blanked = blank_tag(source, tag) if custom else source
        if blanked == source:
            return report["mode"], blockers
        source = blanked
    if not blockers:
        return "native", blockers
    if all(blocker["opts_in"] for blocker in blockers):
        return "native", blockers
    return "custom tags", blockers


def try_isolated(engine, results, compilers):
    """Opt every blocking custom tag in, then compile each blocked template.

    Records on each result whether the real compile stayed native, fell back
    to Django's parser (for example when a tag's body holds `{% block %}`), or
    raised because the compiler needs more than its own body.
    """
    from django_rusty_templates import interop

    tags = {
        blocker["tag"]
        for result in results
        for blocker in result["blockers"]
        if blocker.get("custom") and not blocker["opts_in"]
    }
    for tag in tags:
        for _, compiler in compilers[tag]:
            target = getattr(compiler, "__func__", compiler)
            try:
                target.rusty_isolated_compiler = True
            except (AttributeError, TypeError):
                pass
    fallbacks = []
    original = interop.compile_template

    def record(*args, **kwargs):
        fallbacks.append(args[0])
        return original(*args, **kwargs)

    interop.compile_template = record
    try:
        for result in results:
            if result["mode"] != "custom tags":
                continue
            fallbacks.clear()
            try:
                engine.from_string(result["source"])
            except Exception as error:  # noqa: BLE001 - reported, not fatal
                message = str(error).strip().splitlines()[0]
                result["isolated"] = f"error: {type(error).__name__}: {message}"
            else:
                result["isolated"] = "fallback" if fallbacks else "native"
    finally:
        interop.compile_template = original


def app_templates(app_labels):
    from django.apps import apps

    for config in apps.get_app_configs():
        if app_labels and config.name not in app_labels:
            continue
        root = Path(config.path) / "templates"
        if not root.is_dir():
            continue
        for path in sorted(root.rglob("*")):
            if path.is_file():
                yield config.name, root, path


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("apps", nargs="+", help="apps to install and survey")
    parser.add_argument(
        "--extra-app",
        action="append",
        default=[],
        help="an app to install without surveying its templates",
    )
    parser.add_argument(
        "--setting",
        action="append",
        default=[],
        metavar="NAME=VALUE",
        help="a Django setting, VALUE as a Python literal, such as MIDDLEWARE=['a.B']",
    )
    parser.add_argument(
        "--try-isolated",
        action="store_true",
        help=(
            "opt the blocking custom tags into rusty_isolated_compiler and report "
            "whether each blocked template then compiles natively"
        ),
    )
    parser.add_argument("--json", type=Path, help="write every result here")
    args = parser.parse_args()

    extra_settings = {}
    for setting in args.setting:
        name, _, value = setting.partition("=")
        extra_settings[name] = ast.literal_eval(value)
    configure(args.apps + args.extra_app, extra_settings)
    from django.template import engines

    engine = engines["django"].engine if "django" in engines else None
    engine = engine or next(iter(engines.all())).engine
    compilers = tag_compilers()

    results = []
    for app, root, path in app_templates(set(args.apps)):
        name = path.relative_to(root).as_posix()
        try:
            source = path.read_text("utf-8")
        except UnicodeDecodeError:
            continue
        try:
            mode, blockers = explain(engine, source, name, compilers)
        except Exception as error:  # noqa: BLE001 - reported, not fatal
            mode, blockers = "error", [{"reason": f"{type(error).__name__}: {error}"}]
        results.append(
            {
                "app": app,
                "template": name,
                "mode": mode,
                "blockers": blockers,
                "source": source,
            }
        )

    if args.try_isolated:
        try_isolated(engine, results, compilers)
    print_report(results)
    print_library_tags(own_compiler_tags(compilers, args.apps))
    for result in results:
        del result["source"]
    if args.json:
        args.json.write_text(json.dumps(results, indent=2) + "\n")


def print_report(results):
    by_app = defaultdict(Counter)
    for result in results:
        by_app[result["app"]][result["mode"]] += 1
    modes = ["native", "custom tags", "hybrid", "unsupported", "error"]
    print("Templates by result. Native includes templates whose custom tags all")
    print("opt into rusty_isolated_compiler; 'custom tags' means only custom tags")
    print("that don't opt in block it.\n")
    print(f"{'app':40} " + " ".join(f"{mode:>11}" for mode in modes))
    for app, counts in sorted(by_app.items()):
        print(f"{app:40} " + " ".join(f"{counts[mode]:>11}" for mode in modes))

    blocking = Counter()
    libraries = {}
    reasons = {}
    for result in results:
        for blocker in result["blockers"]:
            if blocker.get("opts_in"):
                continue
            key = blocker.get("tag") or blocker["reason"]
            blocking[key] += 1
            libraries[key] = ", ".join(blocker.get("libraries", [])) or "-"
            reasons[key] = blocker["reason"]
    if blocking:
        print("\nBlockers (templates blocked, library, reason)\n")
        for key, count in blocking.most_common():
            print(f"{count:>5}  {key:30} {libraries[key]:25} {reasons[key]}")
    tried = [result for result in results if "isolated" in result]
    if tried:
        print("\nWith every blocking custom tag opted into rusty_isolated_compiler\n")
        outcomes = Counter(
            "error" if result["isolated"].startswith("error") else result["isolated"]
            for result in tried
        )
        for outcome in ["native", "fallback", "error"]:
            print(f"{outcomes[outcome]:>5}  {outcome}")
        by_tag = defaultdict(Counter)
        for result in tried:
            outcome = result["isolated"].split(":")[0]
            for blocker in result["blockers"]:
                if blocker.get("custom") and not blocker["opts_in"]:
                    by_tag[blocker["tag"]][outcome] += 1
        print(f"\n{'tag':30} {'native':>8} {'fallback':>8} {'error':>8}")
        for tag, counts in sorted(by_tag.items(), key=lambda item: -item[1].total()):
            print(
                f"{tag:30} {counts['native']:>8} {counts['fallback']:>8} {counts['error']:>8}"
            )
        errors = Counter(
            result["isolated"]
            for result in tried
            if result["isolated"].startswith("error")
        )
        for message, count in errors.most_common(5):
            print(f"  {count} x {message[:150]}")
    errors = [result for result in results if result["mode"] == "error"]
    if errors:
        print("\nErrors\n")
        for result in errors:
            print(
                f"  {result['app']}: {result['template']}: {result['blockers'][0]['reason']}"
            )


def print_library_tags(found):
    if not found:
        return
    print("\nTags with their own compiler (* already opts in)\n")
    for library, tags in sorted(found.items()):
        names = ", ".join(sorted(tag + ("*" if opted else "") for tag, opted in tags))
        print(f"  {library}: {names}")


if __name__ == "__main__":
    sys.exit(main())
