# Literal expression and escaping audit (#74)

Upstream reports were checked read-only against this fork on 2026-10-10.

- [Upstream #516](https://github.com/LilyFirefly/django-rusty-templates/issues/516):
  filtered literal template names already work in our include and inheritance
  paths, including loaded custom filters. Differential tests now retain that
  behavior. Literal filters in `if` and `for` did fail here, leading to the
  confirmed fork bug #80 and its lexer fix. Quoted and numeric expressions now
  include their filter chains, keeping operators and `reversed` separate.
- [Upstream #203](https://github.com/LilyFirefly/django-rusty-templates/issues/203):
  our renderer escaped DEL starting with Django 6.1 but missed the rest of the
  C1 range. The old tests used a single escaping expectation across versions,
  including incorrect hexadecimal letter case, and marked mismatches as expected
  failures. The renderer now escapes DEL/C1 from Django 6.1 onward, retaining
  unchanged characters on 5.2/6.0. Tests assert all 33 characters against both
  engines with uppercase escapes where applicable, without expected failures.
- [Draft upstream #57](https://github.com/LilyFirefly/django-rusty-templates/pull/57):
  our addslashes suite already verifies escaping on/off, safe strings, quotes,
  backslashes and non-string values against both engines without expected
  failures. No runtime change is needed.

The new coverage exercises ordinary and inherited expressions, filter chains,
Unicode and quoted whitespace, filter arguments, boolean operators, reversed
and empty loops, and malformed inputs. An empty filter argument also exposed
an existing unchecked character read; it now raises a syntax error rather than
panicking. Filtered translated roots are not treated as valid new syntax:
Django 5.2 rejects those tag roots while splitting translation-marked tokens.
Translated filter arguments remain covered where Django accepts them.

The version-specific rules now match installed Django 6.1.2 and the pristine
6.1/6.2 development sources, while preserving 5.2/6.0 output.
No new backend, native inheritance implementation or broad CI matrix is needed
for these fixes. Findings and implementation remain in the private fork.
