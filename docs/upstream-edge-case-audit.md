# Literal expression and escaping audit (#74)

Upstream reports were checked read-only against this fork on 2026-10-10.

- [Upstream #516](https://github.com/LilyFirefly/django-rusty-templates/issues/516):
  filtered literal template names already work in our include and inheritance
  paths, including loaded custom filters. Differential tests now retain that
  behavior. Literal filters in `if` and `for` did fail here, leading to the
  confirmed fork bug #80 and its lexer fix. Quoted and numeric expressions now
  include their filter chains, keeping operators and `reversed` separate.
- [Upstream #203](https://github.com/LilyFirefly/django-rusty-templates/issues/203):
  our renderer already escapes DEL starting with Django 6.1 and leaves C1
  characters unchanged. The old tests expected different output and marked the
  resulting mismatches as expected failures. They now assert each version's
  actual Django output, removing those artificial expected failures.
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

Changing tests to match Django does not change the underlying escaping rules.
No new backend, native inheritance implementation or broad CI matrix is needed
for these fixes. Findings and implementation remain in the private fork.
