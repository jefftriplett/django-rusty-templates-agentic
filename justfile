set shell := ["bash", "-euox", "pipefail", "-c"]

# List available recipes
_default:
    @just --list --unsorted

# Bootstrap the environment for running the test suite
bootstrap:
    #!/usr/bin/env bash
    set -euox pipefail
    # Compile translations
    cd tests && django-admin compilemessages

# Build the Rust extension and run the Python test suite (extra args forwarded to pytest)
[group('test')]
python-test *ARGS:
    maturin develop
    pytest {{ARGS}}

# Run the Python test suite under coverage
[group('coverage')]
python-coverage:
    maturin develop
    pytest --cov

# Print a terminal Rust coverage report from the Python test suite
[group('coverage')]
rust-coverage:
    #!/usr/bin/env bash
    set -euox pipefail
    cargo llvm-cov clean --workspace
    source <(cargo llvm-cov show-env --sh)
    cargo llvm-cov --no-report
    maturin develop
    pytest
    cargo llvm-cov report --workspace

# Generate an HTML Rust coverage report and open it in the browser
[group('coverage')]
rust-coverage-browser:
    #!/usr/bin/env bash
    set -euox pipefail
    cargo llvm-cov clean --workspace
    source <(cargo llvm-cov show-env --sh)
    cargo llvm-cov --no-report
    maturin develop
    pytest
    cargo llvm-cov report --workspace --open

# Prepare a CalVer bump on a branch and open its PR (start from clean main)
[group('release')]
release-prepare VERSION:
    #!/usr/bin/env bash
    set -euo pipefail
    test -z "$(git status --porcelain)"
    test "$(git branch --show-current)" = main
    git pull --ff-only origin main
    test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
    git switch -c "release/{{VERSION}}"
    uv run --no-sync python scripts/bump_version.py '{{VERSION}}'
    cargo metadata --locked --no-deps --format-version 1 >/dev/null
    git add Cargo.toml Cargo.lock README.md
    git commit -m "Release {{VERSION}}"
    git push -u origin "release/{{VERSION}}"
    gh pr create --base main --head "release/{{VERSION}}" --title "Release {{VERSION}}" --body "Bump the package version to {{VERSION}}. After merging, run just release {{VERSION}} to build wheels and publish to PyPI."

# Tag the merged version and push it to trigger wheel builds and PyPI publishing
[group('release')]
release VERSION:
    #!/usr/bin/env bash
    set -euo pipefail
    test -z "$(git status --porcelain)"
    test "$(git branch --show-current)" = main
    git pull --ff-only origin main
    test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
    uv run --no-sync python -c 'import sys, tomllib; actual = tomllib.load(open("Cargo.toml", "rb"))["package"]["version"]; sys.exit(0 if actual == sys.argv[1] else f"Requested version {sys.argv[1]} does not match {actual}")' '{{VERSION}}'
    cargo metadata --locked --no-deps --format-version 1 >/dev/null
    git tag -a '{{VERSION}}' -m 'Release {{VERSION}}'
    git push origin 'refs/tags/{{VERSION}}'
