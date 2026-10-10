# Variable and context audit (#73)

Upstream was inspected read-only on 2026-10-10. These decisions concern this
fork's implementation, including its Django 6 support and inheritance bridge.

## Adopt parser naming clarity

[Upstream PR #520](https://github.com/LilyFirefly/django-rusty-templates/pull/520)
distinguishes parsing a variable reference from parsing an expression containing
filters. Apply that distinction without requiring its preceding enum refactor:
`parse_variable_reference` selects an ordinary or optimized loop reference;
`parse_filter_expression` parses literals, translations, variables and filter
chains. Both methods are internal. This preserves their existing implementations
and makes the broader expression grammar clear at tag-argument call sites.

## Defer eager conversion of Python values

[Draft PR #75](https://github.com/LilyFirefly/django-rusty-templates/pull/75)
extracts resolved values into integer, float or string variants before filters
and rendering. That cannot be applied directly while retaining Django behavior:

- Python booleans are accepted as integers by numeric extraction. Converting a
  boolean to an integer changes `True`/`False` rendering to `1`/`0`.
- Converting strings to owned Rust strings loses SafeString and custom string
  subclass safety/hooks. It also loses object identity for external filters.
- Native numeric output would bypass Django localization and timezone handling
  currently selected by `format_values` in `src/render/types.rs`.
- A conversion cache must preserve lazy values and live context changes rather
  than cache a stale value across callbacks or render scopes.

The existing #66/#63 improvements instead optimize exact-string output after
filter resolution, preserving Python objects during lookup/filter execution.
No eager-conversion change is proposed, and no performance claim is inferred
from the draft or from coverage changes.

## Evaluate the enum refactor with native inheritance

[PR #519](https://github.com/LilyFirefly/django-rusty-templates/pull/519)
unifies ordinary and optimized loop references and introduces PartsIterator.
Its current diff touches lexer types, parsing, rendering, filters, tags and
tests. It is a prerequisite in upstream's PR #299 history, but does not alone
fix a demonstrated fork bug or establish a rendering improvement. Defer the
broad type migration to the #72 native inheritance experiment, where its value
and conflict surface can be assessed together. Parser naming does not need it.

## Compatibility map

| Concern | Fork implementation / verification |
| --- | --- |
| Dictionary before attribute/index lookup; callable flags/signatures; noisy and silent errors | `lookup_part`, `resolve_callable`, `silent_failure` in `src/render/common.rs`; `tests/test_variable_protocols.py` and `tests/test_variable.py`. Preserve these when evaluating upstream #13. |
| Missing variables and invalid-string formatting | `resolve_output_variable` and `Context::invalid_value`; both literal and filtered output have existing tests. |
| Loop references, parent loops and Django 6 length | `parse_variable_reference`, `ForVariable::resolve`, loop scope metadata; native regions use the live Django dictionary and retain conservative parent-loop boundaries. |
| Filter arguments and scopes | Existing `ArgumentType` variants preserve ordinary/loop references; `tests/test_interop_fragments.py` verifies live loop, with-scope and fallback argument lookup. |
| Safety, lazy values, numeric/date formatting | `Content::render` keeps subclass hooks and non-string Django formatting; filter/safety tests and interoperability formatting tests cover both engines. |
| Django 6.1 double-dot lookup | Version-specific interoperability fallback from #50; a type refactor must retain it and its warnings. |
| Callback mutation and object identity | `Context::resolve_root` reads live contexts; `tests/test_native_regions.py` checks mutations, loop dictionary identity and error cleanup. Do not substitute eager converted values here. |

An open upstream issue is not evidence that these features are absent in this
fork. Any future conversion or enum implementation must pass focused parity
tests and unchanged Django conformance before its release performance is judged.
