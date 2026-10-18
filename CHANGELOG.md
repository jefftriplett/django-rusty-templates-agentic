# Changelog

## Unreleased

## 2026.10.13

Restore callback rendering performance while preserving cyclic collection, and reduce allocations in blocks and includes. ([#171](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/171))

## 2026.10.12

Collect cycles created by custom tags retaining native contexts or block callbacks, including shared scope and loop data. ([#169](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/169))

## 2026.10.11

Release discarded cached engines through Python cyclic GC while preserving retained templates and loaders, and match Django 6+ include-path errors. ([#166](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/166))

## 2026.10.10

Restore Django 6.1 admin rendering with CSP nonce support, complete the default filter set, and fix mixed-template CSRF and HTML handling, Windows template loading, and raw-block parsing. Exercise Rusty directly in the template, admin, and CSP conformance suites. ([#163](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/163))

## 2026.10.9

Make local performance checks less sensitive to allocator state and timing outliers, and remove performance comparisons from GitHub Actions. ([#142](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/142), [#144](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/144))

Reduce rendering allocations, Python lookups, and context copying in loops, filters, components, and template scopes. ([#154](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/154), [#155](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/155), [#156](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/156), [#157](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/157), [#158](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/158), [#159](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/159))

## 2026.10.8

Speed up template parsing and custom tags, reduce string copying and large-context rendering costs, and strengthen performance checks. Match Django’s errors and string behavior for simple-tag results. ([#125](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/125), [#126](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/126), [#127](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/127), [#128](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/128), [#135](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/135), [#136](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/136), [#137](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/137), [#139](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/139))

## 2026.10.7

Reduce Django parsing for DEBUG and mixed inheritance, and parse Django 6 expressions and template partials in Rust. ([#120](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/120), [#121](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/121), [#122](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/122))

## 2026.10.6

Reduce loop and component rendering overhead, with performance checks to catch regressions. ([#106](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/106), [#107](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/107))

## 2026.10.5

Parse and render template inheritance, blocks, and `block.super` in Rust. ([#99](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/99))

## 2026.10.4

Reduce rendering allocations and speed up built-in value lookups. ([#97](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/97))

## 2026.10.3

Reduce include rendering overhead and automate release version bumps. ([#94](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/94), [#95](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/95))

## 2026.10.2

Fix template and context inspection in Django's test client, and add release commands. ([#92](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/92), [#93](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/93))

## 2026.10.1

First Agentic release, with Django 5.2 and 6.x compatibility, template inheritance, custom Python tags, and PyPI wheel publishing. ([#45](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/45), [#46](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/46), [#60](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/60), [#85](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/85), [#90](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/90))
