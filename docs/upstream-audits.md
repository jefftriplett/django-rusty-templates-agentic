# Upstream audit decisions

Upstream research is read-only. All issues, PRs, findings and discussion belong
in this private fork; never comment or post directly to LilyFirefly's project.
An open upstream proposal is an audit candidate, not evidence of a missing
feature or a reason to adopt a change without checking this fork.

## Dependency and tooling audit (#76)

Reviewed on 2026-10-10 against our resolved Cargo dependencies and workflows.

| Upstream proposal | Decision | Evidence |
| --- | --- | --- |
| [num-bigint 0.5 upgrade](https://github.com/LilyFirefly/django-rusty-templates/pull/452) | Defer until PyO3 supports the same bigint series. | Our lockfile resolves num-bigint 0.4.8 and PyO3 0.29.3. PyO3 declares `num-bigint = ^0.4.4`; its Python conversion implementations apply to that crate's BigInt/BigUint types. Our renderer uses those conversions. Upstream's proposed lockfile contains both 0.4.8 and 0.5.1, which does not make the types interchangeable. |
| [Track/group the minimum Rust version](https://github.com/LilyFirefly/django-rusty-templates/pull/542) | Keep the existing toolchain tracking; defer automatic minimum-version upgrades. | Our toolchain is 1.99.0 and our declared minimum is 1.98. They serve different purposes. Raising the minimum with every toolchain release would drop compiler compatibility without evidence that a new language or library feature requires it. |
| [cargo-binstall 1.25.3](https://github.com/LilyFirefly/django-rusty-templates/pull/543) | Adopt the pinned action update in the Rust and lint workflows. | Both workflows install tools through cargo-binstall. GitHub's `v1.25.3` tag resolves to `3be4ad2b146f3302f92e5956c54fedbb6ae8cccc`. The Python sanity workflow does not use this action. |

We do not use num-bigint's random-number features or `RandBigInt`/`BigRng`
interfaces, so its random API migration provides no benefit to this fork.
Revisit the bigint upgrade when PyO3 supports 0.5; check that Python conversion
implementations and our numeric rendering use a single compatible crate series.

Default CI stays Ubuntu 24.04/Python 3.13 with Django 5.2, 6.0 and 6.1.
Broader OS/Python checks remain manually dispatched. Codecov uploads remain
disabled. The action update does not enable or dispatch either manual workflow.
