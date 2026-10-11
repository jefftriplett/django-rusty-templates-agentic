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

# Bump the CalVer version on a release branch and open its PR (extra args go to bumpver update)
[group('release')]
release-prepare *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    test -z "$(git status --porcelain)"
    test "$(git branch --show-current)" = main
    git pull --ff-only origin main
    test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
    uvx bumpver@2026.1132 update {{ARGS}}
    eval "$(uvx --quiet bumpver@2026.1132 show --no-fetch --environ)"
    # bumpver can't target Cargo.lock's package entry; let Cargo refresh only the workspace crates.
    cargo update --workspace --offline
    cargo metadata --locked --format-version 1 >/dev/null
    git switch -c "release/$CURRENT_VERSION"
    git add bumpver.toml Cargo.toml Cargo.lock README.md
    git commit -m "Release $CURRENT_VERSION"
    git push -u origin "release/$CURRENT_VERSION"
    gh pr create --repo jefftriplett/django-rusty-templates-agentic --base main --head "release/$CURRENT_VERSION" --title "Release $CURRENT_VERSION" --body "Bump the package version to $CURRENT_VERSION. After merging, run just release to build wheels and publish to PyPI."

# Tag the merged version and push it to trigger wheel builds and PyPI publishing
[group('release')]
release:
    #!/usr/bin/env bash
    set -euo pipefail
    test -z "$(git status --porcelain)"
    test "$(git branch --show-current)" = main
    git pull --ff-only origin main
    test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
    eval "$(uvx --quiet bumpver@2026.1132 show --no-fetch --environ)"
    cargo metadata --locked --format-version 1 >/dev/null
    git tag -a "$CURRENT_VERSION" -m "Release $CURRENT_VERSION"
    git push origin "refs/tags/$CURRENT_VERSION"
