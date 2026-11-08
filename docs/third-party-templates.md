# Third-party template libraries

`scripts/survey_templates.py` reports how much of a package's templates Rusty
compiles natively, which tags block the rest, and which of the package's tags
make a user's template fall back to Django's parser. Run it in an environment
where the packages are installed, for example:

```bash
python scripts/survey_templates.py --try-isolated crispy_forms crispy_bootstrap5
python scripts/survey_templates.py --try-isolated \
    --setting "MIDDLEWARE=['allauth.account.middleware.AccountMiddleware']" \
    allauth allauth.account allauth.socialaccount allauth.mfa allauth.usersessions
```

`explain_template()` stops at the first blocker and never runs a custom tag's
compiler, so it reports every tag with its own compiler, even one that opts
into `rusty_isolated_compiler`. The survey records each blocking tag, blanks it
out and inspects again, so each template lists all of its blockers.
`--try-isolated` then opts every blocking tag in (only in that process) and
compiles each blocked template for real, reporting whether it stays native.

## Results (November 2026, Django 5.2)

| Package | Version | Templates | Native | Blocked by the package's own tags | Native once those tags opt in |
|---|---|---|---|---|---|
| django-crispy-forms + crispy-bootstrap5 | 2.7 / 2026.9 | 42 | 31 | 11 | 11 |
| django-allauth | 65.21.1 | 169 | 96 | 73 | 72 |
| Wagtail | 8.1 | 548 | 457 | 91 | 82 |
| django-debug-toolbar | 8.0.0 | 27 | 27 | 0 | - |
| django-compressor | 4.6.0 | 6 | 6 | 0 | - |
| django-widget-tweaks | 1.5.1 | 0 | - | - | - |

Every blocker found is a tag with its own compile function. None of these
templates needs a Rusty change to compile, apart from one Wagtail template that
used `default:_("Not started")` inside `{% with %}`, which Rusty rejected until
[#358](https://github.com/jefftriplett/django-rusty-templates-agentic/issues/358).
The templates that still fall back after opting in have a `{% block %}` inside
a custom tag's body, which makes Django's parser compile the whole template.

Tags with their own compiler, which make any template that uses them fall back
to Django's parser:

| Package | Tags |
|---|---|
| django-crispy-forms | `crispy`, `crispy_field`, `specialspaceless` |
| django-widget-tweaks | `render_field` |
| django-compressor | `compress` |
| django-allauth | `element`, `slot`, `setvar` |
| Wagtail | `component`, `dialog`, `dropdown`, `dropdown_button`, `field_row`, `fragment`, `help_block`, `panel`, `rawformattedfield`, `include_block`, `image`, `picture`, `srcset_image`, `get_settings`, `wagtailcache`, `wagtailpagecache` |

With `crispy`, `crispy_field`, `specialspaceless` and `render_field` opted in,
`{% crispy form %}`, `{{ form|crispy }}` and `{% render_field %}` render the
same HTML as Django for a form with text, checkbox and radio fields, with no
fallbacks. A clean compile alone doesn't show a tag renders correctly, so the
other tags need the same kind of check before anyone opts them in.

Until a package opts in itself, a project can opt a tag in from an
`AppConfig.ready()`:

```python
from widget_tweaks.templatetags import widget_tweaks

widget_tweaks.register.tags["render_field"].rusty_isolated_compiler = True
```
