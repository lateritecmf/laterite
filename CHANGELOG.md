# Changelog

Notable changes to the Laterite crates. The crates share one version and release
together. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/) as Cargo reads it: before
1.0 a minor bump is breaking and a patch is additive.

## [Unreleased]

### Added

- `lat` commands find the application from any subdirectory (the nearest
  `config/default.toml`), and `lat admin` reads the database URL from its
  configuration when neither `--database-url` nor `DATABASE_URL` is set.
- `app.env_prefix` declares the environment-override prefix once, for the
  application and every `lat` command.
- The MIT and Apache-2.0 license texts ship in the repository.
- `laterite_core::record`: a `Record` attribute bag with typed values
  (`AttrValue`), typed accessors and a `deserialize` struct view, plus the
  `ModelListener` lifecycle trait and its registration.
- Admin form writes run through a listener pipeline: a module contributes a
  `ModelListenerReg` and its listener can change the record before the write or
  refuse it with per-field messages, and react after it.
- `SaveError` implements `Debug`.

### Changed

- **Breaking**: a form write runs in one transaction the framework owns.
  `Persister::create` and `update` take a `SaveCx` (that transaction) and a
  `Record` instead of a database handle and a text map, and a persister no longer
  opens its own transaction. `rec.to_text_map()` gives the previous value shape.
- **Breaking**: `ModelListener::before_save` takes the same `SaveCx`, so a
  listener reads the in-flight state. `after_save` still takes the database and
  still runs after the commit.
- On an update a persister may supply the pre-write row (`Persister::load`), and
  listeners read it through `Record::original`, `has_original` and `changed`.
- **Breaking**: every save states its `Actor`, either a signed-in `User` or the
  `System` process performing it, reaching listeners and the persister. A write
  with no person behind it names its process rather than borrowing a user.
- **Breaking**: `ModelListener::after_save` takes a `SavedCx` (the pool and the
  actor) rather than a database handle, so it can gain context after 1.0.
- The framework audits descriptor-form writes through a listener rather than a
  call in each handler, so a new descriptor screen is audited by existing.
  Screens that write through their own store functions keep their explicit
  calls.
- **Breaking**: a module's admin screens mount under its identity by default, so
  `rainmill.location` contributing `/nodes` serves `/rainmill/location/nodes` and
  two plugins cannot collide. `Module::admin_base` declares a shorter base, and
  `[backend.paths]` lets a deployment move a module or one of its screens without
  forking it. Two claims on one path abort the boot naming both.
- A module can mount its own admin screens: contribute a `ScreenReg` and the
  framework nests its routes under the module's resolved path, wrapped in the
  session, the permission guard it declares, CSRF and the styled error pages. The
  screen reads a narrow `ScreenCtx` and builds self-links through `ctx.url`, so a
  moved screen moves its links with it.
- A `timestamps` flag on a form stamps `created_at` and `updated_at`, leaving a
  supplied `created_at` alone so an import keeps its history.
- The Model Listeners guide.
- **Breaking**: `router` takes the model-listener contributions. Applications
  boot through `Bootstrap` and are unaffected.
- The built-in persister binds typed values (integers, booleans as integers,
  timestamps and JSON as text) and writes attributes a listener added, not only
  the descriptor's own columns.

### Fixed

- `lat doctor` and `lat serve` used a fixed environment prefix, so their overrides
  missed an application with its own.

## [0.3.0] - 2026-09-05

### Breaking

- Applications boot through `laterite_admin::Bootstrap` with a `ModuleRegistry` of
  `Module` implementations; `router` takes the module contributions, an
  `AdminConfig`, and the catalog store.
- `AdminConfig`, `AppMeta`, and `BackendConfig` are `#[non_exhaustive]`: build one
  with `Default` and set fields. Struct literals no longer compile outside the
  framework, and a field added in a later release is additive.
- Descriptor labels (lists, forms, resources, settings items, permissions) are
  `Text` values built with the `t!` macro; a bare `String` no longer compiles.

### Added

- Module system: the `Module` trait with bundled migrations run in dependency
  order, a registration `Registry` for descriptors and hooks, and database
  capability declarations checked at boot.
- Plugin platform: `plugins/<author>/<plugin>` discovery with a generated manifest
  (`lat plugin sync`), enable and disable from config and the admin, and boot
  isolation with quarantine and a crash-loop journal.
- Localization: the deferred `Text` value with `t!`, `tn!`, and `tp!`; a
  per-request translator resolving the operator preference, `Accept-Language`,
  and `app.locale`; a language picker; PO catalogs contributed by modules; the
  `xx` pseudo-locale; localized dates; and `lat i18n extract`, `check`, `update`,
  and `status`.
- Audit log: an append-only record of every privilege and data mutation, with a
  read-only view under System behind `backend.view_audit_log`.
- Admin sessions with layered CSRF protection and dismissible flash toasts.
- A validation engine with typed rules (required, length, email, url, unique) and
  per-field errors re-rendered with a 422.
- Typed admin errors with styled pages, logged through `tracing`; `app.debug`
  shows the cause.
- Field types as an open registry with a view-model render seam: text input types
  (email, tel, number, url with a copy button), select, and a reference picker
  with a searchable combobox; form writes delegated to registered persisters; a
  `ColumnType` registry for list cells.
- Admin assets: htmx, a `laterite.js` widget lifecycle, and an open asset registry
  with per-page widget assets.
- CLI: `lat serve`, `lat domain` for local wildcard domains, and
  `lat make:migration`.
- Config: `server.listen`, `app.url`, `app.locale`, `app.debug`, and
  `backend.path` to relocate the admin panel.
- Core helpers: `insert_returning_id_on` for transactional inserts and
  `AnyRowExt::get_int`.

### Fixed

- Absent form values persist correctly, and the migration scaffold declares its
  primary key.
- Form field options resolve once at router build.
- Locale names and date formatting assume no particular language; the admin's
  locale set is the loaded catalogs.

### Security

- A resource with a create or edit form must declare a permission; boot aborts
  otherwise.
- The reference-picker search escapes LIKE wildcards.

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
