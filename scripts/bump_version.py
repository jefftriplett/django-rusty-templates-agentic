"""Update the package's CalVer version and its documentation/lockfile."""

import re
import sys
import tomllib
from pathlib import Path


def main():
    version = sys.argv[1]
    if not re.fullmatch(r"[0-9]{4}\.(?:[1-9]|1[0-2])\.[1-9][0-9]*", version):
        raise SystemExit("Use YYYY.M.INCR (for example, 2026.10.2).")
    root = Path(__file__).resolve().parent.parent
    manifest = root / "Cargo.toml"
    current = tomllib.loads(manifest.read_text())["package"]["version"]
    if tuple(map(int, version.split("."))) <= tuple(map(int, current.split("."))):
        raise SystemExit(f"Version must be newer than {current}.")
    replacements = {
        manifest: (f'version = "{current}"', f'version = "{version}"'),
        root / "Cargo.lock": (
            f'name = "django-rusty-templates"\nversion = "{current}"',
            f'name = "django-rusty-templates"\nversion = "{version}"',
        ),
        root / "README.md": (
            f"The current package version is `{current}`.",
            f"The current package version is `{version}`.",
        ),
    }
    updates = {}
    for path, (old, new) in replacements.items():
        source = path.read_text()
        if source.count(old) != 1:
            raise SystemExit(f"Expected exactly one version entry in {path.name}.")
        updates[path] = source.replace(old, new, 1)
    for path, source in updates.items():
        path.write_text(source)
    print(f"Updated {current} to {version}.")


if __name__ == "__main__":
    main()
