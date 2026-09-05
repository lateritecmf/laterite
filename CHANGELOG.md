# Changelog

Notable changes to the Laterite crates. The crates share one version and release
together. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/) as Cargo reads it: before
1.0 a minor bump is breaking and a patch is additive.

## [Unreleased]

### Breaking

- `AdminConfig`, `AppMeta`, and `BackendConfig` are `#[non_exhaustive]`: build one
  with `Default` and set fields. Struct literals no longer compile outside the
  framework, and a field added in a later release is additive.

## [0.2.0] - 2026-08-21

### Breaking

- The settings crate was folded into `laterite-admin`; `laterite-settings` is no
  longer published.

### Added

- `lat new`, an interactive installer that produces a working application, with a
  `--framework-path` dev mode; `lat doctor` health checks.
- `laterite-web`, the public web layer with static-site generation.
- An application name that drives the admin brand.
- Per-crate READMEs generated from the crate doc comments.

## [0.1.0] - 2026-08-19

### Added

- `laterite-core`: config, errors, the portable multi-database data layer
  (Postgres, MySQL, SQLite), and the migration engine.
- `laterite-auth`: users with Argon2id passwords, sessions, roles, permissions
  with per-user overrides, and a first-run setup flow.
- `laterite-admin`: descriptor-driven lists, forms, and settings screens in the
  Laterite design system, with per-user display timezones.
- `laterite-cli`: the `lat` command.
