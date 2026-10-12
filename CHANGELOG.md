# Changelog

## Unreleased

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
