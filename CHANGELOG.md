# Changelog

Notable changes to the Laterite crates. The crates share one version and release
together. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/) as Cargo reads it: before
1.0 a minor bump is breaking and a patch is additive.

## [Unreleased]

### Added

- `laterite_core::queue`: a `Job` is a serde struct behind `registry.add_job::<T>()`, pushed with
  `ctx.queue()`, run by in-process workers with tries, backoff, timeout and a failed list; `[queue]`.
- `Bootstrap::run` (serve, or `queue work` alone; the scaffold calls it), `lat queue work | list |
  failed | retry | forget | flush`, the `queue.job_failed` event.
- `laterite_core::schedule`: cron entries registered with `registry.add_schedule`, evaluated each
  minute in their timezone on one process, `[schedule]`, `lat schedule run | list`; the hourly
  session purge is one.
- The Jobs and Scheduled Tasks screens under Settings → System, behind `backend.manage_jobs` and
  `backend.manage_schedule`.

## [0.9.22] - 2026-10-23

The Cache slice of the Services theme: values kept under the namespaces they were
built from and dropped by every framework write, in every process.

### Added

- `laterite_core::cache`: values kept for a lifetime under the namespaces they were built from,
  bumped by every framework write; `memory`, `null` and `redis` stores; `[cache]`; `RouteCtx::cache`.
- Write handles that bump: `RouteCtx::settings`, `laterite_web::theme::Themes`; the parameters
  handle now writes through the cache. `lat cache clear | list`.

### Fixed

- A click anywhere in the admin no longer cycles the colour mode: the mode button has its own
  hook, so the root's default-mode attribute is not mistaken for it.

## [0.9.21] - 2026-10-23

The Crypt slice, first of the Services theme: an application key everything
seals and signs under, a sensitive field type, key rotation, and the handbook
restructured by task.

### Added

- `laterite_core::crypt`: the application key (`app.key`, `lat key generate`) seals values,
  signs links and mints expiring tokens per purpose; `RouteCtx::crypt`. Signed media URLs use it.
- The `sensitive` field type: a secret sealed at rest, shown as a mask, revealed on request with an
  audit entry. `lat crypt rotate` re-seals every such column after `lat key generate --rotate`.

### Fixed

- An edit writes only the attributes the record carries, so a value a field type leaves out (a
  blank password) keeps what is stored; it was written as NULL.
- The context side panel folds at phone and tablet widths as documented, and a colour input is
  a swatch, not a full-width bar.

## [0.9.20] - 2026-10-22

The Admin polish slice: dashboard widgets each operator arranges, brand marks
and colours with a customised sign-in, skins, a parameters store, a context
side panel, checked uploads with a cleaned SVG, and snippets backed by
components.

### Fixed

- The colour-mode boot script is served as a file and loaded blocking, so the admin's content
  security policy admits no inline script at all.

### Added

- Brand settings: logo and browser icon from the library, an accent colour applied through the
  palette variables, a default colour mode, the sign-in prompt, tagline, background and panel, and
  a custom stylesheet cleaned on display. A `color` input for text fields.
- Dashboard widgets: `laterite_admin::widget::Widget` with `AdminRegistry::add_widget`, permission per
  widget, sizes, timed refresh; each operator arranges their own under Manage widgets. Four built in.
- Skins: `backend.skin` selects `standard`, `compact` or a module's own, registered with
  `AdminRegistry::add_skin`; `add_asset` serves a module's file under the admin's assets.
- Parameters store: `laterite_core::parameters` keeps a module's own state under
  `namespace::group.item` keys, cached per process through `RouteCtx::parameters`.
- `PageHead::aside`: a context side panel beside a page's body that folds under its label at
  phone and tablet widths. The theme pages list their templates in it.
- Uploads are checked against `backend.upload_extensions` (a default list of media, document,
  font and archive types) and an SVG is cleaned to a drawing (`laterite_media::sanitize`).
- Component-backed snippets: `ComponentReg::snippet` lists a component in the Insert menu and a
  page runs it where its figure stood. A partial declares its snippet through fields.

## [0.9.19] - 2026-10-21

The admin verified at phone and tablet widths and from the keyboard, with a
visual pass over every component: toasts on the dark surface, outline danger
buttons, banded list footers, quiet editable cells, contrast tokens, labels,
a skip link, a focus-trapped drawer, roving toolbars, and browser hardening
headers on every response.

### Added

- Security headers on every admin response (a content security policy with no inline script,
  `nosniff`, frame, referrer, permissions and opener policies; HSTS with `secure_cookie`).
- A skip link, a focus-trapped drawer, and single-tab-stop toolbars with arrow-key movement.
- Labels on editable cells, value-form fields and the palette input; arrow keys in the Insert menus.

### Fixed

- Text tokens under the 4.5 contrast ratio raised (hint text, accent, attention) and dark mode given
  readable success, danger and info tones.
- Flash toasts, danger buttons, list footers, editable cells and the error page restyled to the
  palette; settings list inline, larger tap targets and split panes kept to the screen on phones;
  media filters after the content.

## [0.9.18] - 2026-10-21

The editors: rich text over TipTap and ProseMirror, markdown and code over
CodeMirror 6, built once from a lockfile into bundles the admin ships, with
schema-clean HTML on save, a server-rendered markdown preview, and an Insert
menu that modules and themes extend with commands and snippets.

### Added

- `richeditor` field type (TipTap over ProseMirror; `rows`, `placeholder`, `toolbar`): HTML kept to
  the editor's schema on save, library images linked to the record, `lat.mediaPicker` shared.
- Editor contributions: `AdminRegistry::add_editor_command` and `add_snippet_source` feed both
  editors' Insert menu; a theme partial's `snippet` setting makes it a block, expanded by `|content`.
- `markdown` field type (CodeMirror 6 with a server-rendered Preview tab), `laterite_core::markdown::render`
  as the one renderer, and a `markdown` filter for themes.
- `codeeditor` field type (CodeMirror 6; `language`, `rows`, `line_numbers`, `wrap`) and
  `PageHead::assets` with `CODE_EDITOR_ASSETS`; the theme template editor uses it.

## [0.9.17] - 2026-10-16

Ordered hierarchies: one arrangement engine for every list modules insert into,
with named slots that say where a thing goes, deployment overrides and moves in
configuration, and each operator's own order and placement on top; Themes under
Settings.

### Added

- `laterite_core::arrange`: one engine for every list entries insert themselves into,
  with named slots, anchors, a per-hierarchy policy and `[backend.order]` overrides.
- `slot` on `menu:` placements and settings items; the top bar and the settings index
  arrange by it, so a plugin's resource lands after Media by default.
- `[backend.menu]`: an entry moved between the top bar, a section and the settings
  index, keeping its label, icon, permission and target; the boot log prints every
  arrangement.
- An operator's own menu: the settings list drags like the tabs, and Preferences
  lets them show any entry in the navigation or in settings; Reset puts it all back.

### Changed

- Themes sits in the settings index under Website; the top bar carries Dashboard, Media,
  the application and Settings.

## [0.9.16] - 2026-10-16

The media library: folders, links and the browser, private-disk serving behind a
signed URL with `lat media gc` and `verify`, named-preset variants, the `media`
field with its picker, an S3-compatible disk behind a feature, cropping, and
direct uploads through a quarantine prefix; the Themes and Media screens laid out
on dialogs, panes and attached toolbars, with the uploader widget.

### Added

- The media library: the Media tab with virtual folders, upload, search, a kind filter
  and sorting, a file page with who links to it, and `media.*` permissions.
- `laterite_media::{folder, link, library}`: folders as rows with materialized
  paths, links by owner and field, and listings; `DiskReg::private`.
- Every file serves at `/storage/media/<hash>/<name>` with an ETag, `immutable`
  caching and `nosniff`; a private disk's file only at a signed, timed URL
  (`medialib::signed_file_url`, key `backend.secret` or one generated once).
- `lat media gc` and `lat media verify` (`laterite_media::{gc, verify}`), and
  `StorageDriver::signed_url` for a store that presigns.
- `PageHead` with `Shell::page_with` (a back link and actions beside the title),
  `dialog.lat-dialog` opened by `data-lat-dialog`, and the `lat-split`/`lat-pane` layout.
- Direct uploads: `/admin/media/direct` presigns a PUT under a quarantine prefix on a
  disk that can, the uploader puts the bytes there, and `/direct/complete` hashes,
  promotes and records them; `StorageDriver::{presigned_put, copy}` with defaults.
- Cropping on the file page: a drawn area saved as `<stem>-crop.<ext>` beside the
  original or written in its place (`image::crop`, `record::replace_blob`).
- `S3Disk` behind the media crate's `s3` feature (`lateritecmf/s3`): any S3-compatible
  store through an endpoint override, multipart past 16 MiB, presigned GET and PUT.
- The `media` field type (`mode`, `max`): a picker over the library, ids stored,
  the record's links written on save; `media()` and `|variant` in templates.
- Variants: `Preset` registered on the registry, rendered on first request at
  `/storage/media/<hash>/<preset>/<name>`, with `thumb` and `preview` built in.
- Uploads lose their EXIF before hashing; `width`, `height` and `meta.orientation` are recorded.
- The uploader (`_upload.html`, widget `upload`): a drop zone that sends each file itself
  with progress, or stands as the file field of a form; the media upload answers it in JSON.

### Changed

- Every list is one card under its heading: the toolbar band on top with Import and
  Export behind a `⋯` menu, the filter band below it, then the table; a page's actions
  render in a band under the heading.
- A theme's name, author, homepage, description and the new `preview` URL are edited
  in a properties dialog; theme.yaml itself travels in the zip and is no longer a textarea.
- The Themes and Media screens: a theme selector with a Manage menu, a template
  navigator beside the editor, and a media browser with a filter pane; forms open in dialogs.

### Fixed

- A blank optional non-text field (a cleared reference or media id) saved on Postgres;
  the persister bound a text-typed null that a bigint column refused.

## [0.9.15] - 2026-10-16

Components: a module's compiled units attached to a page or layout by data,
run before the render and read under their alias, with their own markup a
theme may override; markup a module adds to every engine; maintenance mode;
and the session cookie covering the site so an operator is known to it.

### Added

- `theme::markup::MarkupReg`: a module's functions, filters and globals installed
  into every theme engine; `build::build_with` takes them for a static build.
- Maintenance mode (Settings › Website, `web.manage_site`): visitors get the theme's
  `/maintenance` page or a notice with 503; an operator who may manage themes sees the site.

- `theme::component`: a `Component` trait with declared properties, registered as a
  `ComponentReg`; attached in a page's or layout's `components` settings, run before
  the render and read under the alias, with `{{ :name }}` taking a URL parameter.
- `site::render_page_with` and `site::run_components`; `build::build_with` takes the
  registered components too.
- `Component::markup` and `component("alias")`: a component's own markup, overridden
  by the theme's `partials/<alias>/default.htm`; the page editor's Components box.

### Changed

- With a site registered, the session and remember cookies are scoped to `/` rather
  than the admin mount, so the site recognises a signed-in operator.

## [0.9.14] - 2026-10-16

Pages and layouts: the public site served from the active theme. A page's URL
is a pattern, its layout a setting, its settings typed fields in the editor;
the site renders with `this` and `head`, the theme's own 404 and error pages,
assets with an ETag, and preview for an operator; `lat theme build` writes the
same pages as a static site.

### Added

- Pages route: a page's `url` pattern (`:param`, `?`, `?default`, `|regex`, `*`),
  its `layout` wrapping it on save, `page()` for links, and `Engine::resolve`.
- The public site: the active theme's pages served at every URL the admin leaves,
  with `this` and `head`, a `/404` and `/error` page, text assets with an ETag, and
  preview of an operator's edit theme and hidden pages.
- `PublicRouteReg::site`, the route that takes the panel's 404 for the site;
  `RouteCtx::operator` and `RouteCtx::record_unserved` for a public route.
- The template editor shows a page's settings as typed fields with a layout picker,
  and the theme page lists each page's URL.
- `partial()` carries the request's `this` and `head`; `content()` takes variables
  for `{name}` in the block; `asset()` and `url()` build theme and site links.
- `lat theme build` and `theme::build`: a theme's fixed pages and assets rendered to a
  directory for a static host, through the render path the site serves.

## [0.9.13] - 2026-10-16

Themes: the public site's templates as rows edited from the admin. A theme is a
YAML config and templates by kind, resolved through a parent chain and rendered
by a runtime engine; every save keeps a revision; settings declared in the
config; export and import as a zip. Plus the umbrella crate on crates.io and
`lat new` generating on it.

### Added

- `laterite_web::theme`: themes as rows with a YAML config, templates by kind and
  path resolved through a parent chain, a revision per save, and the active and edit theme.
- `laterite_web::theme::Engine`: a theme's templates rendered with layouts, partials
  and content blocks, a parse check for the editor, and a cache rebuilt on change.
- The Themes tab (`web.manage_themes`): themes created, duplicated, extended, locked,
  activated and deleted, and their templates edited with a parse check and revisions.
- `RouteCtx::audit`, so a module's own screen writes the audit log like the admin's.
- Theme settings: fields declared under `form` in a theme's config, edited on its
  page and read in templates as `theme.<field>`; a child theme uses its parent's form.
- `ValueForm` and `RouteCtx::value_form`: a form over a JSON object through the
  registered field types, for a module's own screen; the settings screens use it too.
- `lat theme export`, `lat theme import` and `lat theme list`, and Export and Import on
  the Themes screen: a theme as a zip of `theme.yaml` and one file per template.

### Changed

- `lateritecmf` is on crates.io, and `lat new` generates an application on it:
  `use lateritecmf::prelude::*;` with the driver feature set on the umbrella.

### Fixed

- `lat make:migration` listed the new migration inside the module impl that
  follows the set in a generated application; it goes into the set.

## [0.9.12] - 2026-10-15

Navigation and system screens complete, closing the admin panel: a menu
registry with sections, anchors and the operator's own order, breadcrumbs, a
command palette, a request log, resource hooks, a module boot hook, typed UI
elements and per-operator settings.

### Added

- A menu registry: `menu: { under, icon, order }` (or `false`) on a resource,
  `ScreenReg::menu`, and `NavItem` sections and entries contributed by a module.
  Every entry carries its target's permission, so an operator sees only what
  they may open; a section's entries show in the context sidebar.
- `after` and `before` on a menu placement or item: sits beside that sibling
  whatever the orders say, so two modules never fight over a number.
- An operator drags the top bar's tabs into their own order, kept with their
  preferences; "Reset the menu order" on Preferences restores the default.
- Breadcrumbs on a section's pages and on settings pages, derived from the menu.
- A command palette (the search button, Ctrl+K): menu entries, settings and,
  for a resource with `palette: <picker source>`, its records, opened at their edit page.
- A request log: every URL the admin could not serve, counted, under Settings
  › System for `backend.view_request_log`; it keeps its most recent two thousand rows.
- `ResourceHook`: a module changes another module's resource before it mounts,
  adding a column, a field, a relation or a placement to a screen it does not own.
- `Module::boot`: runs once after every module has migrated and registered, with
  the database and the registry; the auth module's first sweep of expired sessions is one.
- `laterite_admin::ui`: typed buttons, toolbars, badges and cards for a screen that
  composes a page by hand, rendered through the shell.
- `PreferencesModel` and `SettingsItem::per_operator`: a settings model stored
  once per operator and edited from their Preferences page, with `load_for` and `save_for`.

### Changed

- The backend users screen is titled "Administrators", as its settings entry
  always was.

### Fixed

- On a narrow screen the section sidebar sat above the content and pushed it
  down; its entries now nest under the active tab in the menu drawer.
- Deleting rows from a list returned to a doubled path (`/admin/admin/...`), a 404;
  it returns to the list.
- The setup screen's first sign-in now records the client's address and agent
  like every other sign-in.
- The list page's filter triggers were unparsable for htmx (a syntax error on
  every list); a filter change now swaps the table in place as intended.

## [0.9.11] - 2026-10-13

Relations on Postgres: a foreign key and a picker's choice are stored as numbers.

### Added

- `id`, a field type for a record id held as a number: a hidden foreign key or a
  plain key column with no picker.

### Fixed

- A relation's foreign key and a `reference` field's choice were written as text,
  which Postgres refuses for a bigint column; both are stored as numbers now.
- The main pane keeps its width when its content grows past the fold: the
  scrollbar's space is reserved. Row actions are one group of matching buttons.

## [0.9.10] - 2026-10-13

Relations and structure complete: related records on the edit page, lists kept
in their own order, tree lists, and cells edited in the row. Plus `lateritecmf`.

### Added

- `relations` on a resource: has-many (New, Edit, Delete) and many-to-many through
  a pivot table (Attach from the picker, pivot fields, Remove), as edit-page sections.
- "Save and continue editing" beside Save on a resource with relations.
- `reorder` on a list: rows kept in an order of their own, dragged by a grip or
  moved by arrows, with sparse keys so a move rewrites one row.
- `tree` on a list: rows shown as a tree on their parent column, children
  loading in place, and "Move to" through `Persister::move_to` (default writes the column).
- `editable: true` on a list column: the cell is the form field's control,
  saved on change through the form's rules, persister and listeners.
- `lateritecmf`, the one crate an application depends on: it re-exports the
  family as `lateritecmf::{core, auth, admin, media, web}`, forwards the driver
  features and carries a prelude.

### Changed

- Import is part of `laterite-admin`: `import: true` on a resource needs no
  other crate, module or disk; uploads go under `backend.storage`. The
  `laterite-import` crate, never published, is gone.
- `laterite-media`'s table migrates with the admin; registering its module is
  no longer needed.

## [0.9.9] - 2026-10-07

Bulk import complete: a CSV through a resource's form, checked first, with a
report and the refused rows back as a file.

### Added

- New crate `laterite-import`: runs a CSV through a resource's form, detached,
  with a polled report of the rows created, updated, skipped and refused.
- `FormHandle::load(id)` returns a record's values as its edit form shows them.
- `RouteCtx::form(path)` hands a screen a resource's form to write records
  through from code, with the submit's rules, listeners, persister and audit.
- A resource declaring `import: true` gets import pages under its path and an
  Import button beside its export menu: a CSV in, columns matched (remembered
  from the last file with the same titles), a check that writes nothing, then
  the run, its live report, and the refused rows back as a file to fix.
- `laterite_admin::Shell` is public: a contributed screen renders its own
  templates inside the admin chrome with `Shell::page`. A module mounts routes
  under a resource through `ResourceFeatureReg`.
- `laterite_media::DiskReg` declares a named disk for modules that store files.
- A CSV export starts with a byte-order mark, so a spreadsheet reads it as UTF-8.

### Fixed

- `laterite-media`'s table migrates on Postgres and MySQL: its `in_library`
  default and binding were booleans on the integer column.

## [0.9.8] - 2026-10-01

### Fixed

- A repeater on a descriptor form shows its stored rows on edit (saving the
  form had emptied them) and keeps typed rows when a save is refused.

## [0.9.7] - 2026-09-30

The repeater: rows move, copy and collapse to a list by themselves, and every
control reads in the operator's language.

### Added

- A repeater row moves up and down and copies into a new row (`reorder`,
  `duplicate`), and past three fields a row the rows collapse to a list
  (`display: auto`).

### Fixed

- A repeater's Add, Remove and Untitled read in the operator's language, and a
  widget inside an added row now starts.

## [0.9.6] - 2026-09-30

Lists that keep their place: a row opens its record, a range of rows ticks
with Shift, the query an operator used comes back, and stored codes read as
words.

### Added

- A list row opens its record, Shift and a click ticks a range of rows, the
  query an operator used comes back (`remember`), and a column's `labels` name
  its stored codes; `row_click: none` keeps rows plain.

## [0.9.5] - 2026-09-30

Pickers: a long dropdown is searched, and the record picker takes the
keyboard.

### Added

- A dropdown past ten choices is searched (`search`), and the record picker
  takes the keyboard: Down and Up, Enter, Escape, an emptied box clears it.

## [0.9.4] - 2026-09-30

Field dependencies: a slug follows its title, and a field shows, hides or
fills itself as another field changes, all from the descriptor.

### Added

- `preset` fills a field from another as it is typed (a slug from a title), and
  `trigger` shows, hides, enables, empties or fills a field as another changes.

## [0.9.3] - 2026-09-30

Everyday form behaviours: a form asks before losing work, opens with the cursor
where typing starts, counts against a length limit, grows its textareas and
shows a password on request.

### Added

- Forms ask before leaving with changes unsaved and place the cursor where
  typing starts: `confirm_leave`, `focus`.
- A `max_length` rule sets `maxlength` and shows a count: `counter`. A textarea
  grows with its text: `grow`. A password field shows what was typed: `reveal`.
- `lat.confirm(text, labels, done)` asks in the admin's dialog from script.

## [0.9.2] - 2026-09-30

The checklist: one component for every list of boxes, which offers select all,
search and closed groups by itself as the list grows.

### Added

- A `checklist` field type and `laterite_admin::checklist`: boxes in groups, with
  select all, search and settled groups closed once the list is long.
- `FieldType::submitted` gathers a field of several controls into one value.

### Changed

- The Roles permission editor and role assignment are checklists.

## [0.9.1] - 2026-09-30

Events: every admin island announces what it does and can be driven from
script, and modules hear each other's facts on an application event bus.

### Added

- Every admin island announces what it does as a `lat:<component>:<event>` DOM
  event and hands back a controller: `lat.on`, `lat.emit`, `lat.get`.
- An event bus: `registry.listen::<E>(..)` hears a typed event, `events.emit(..)`
  announces one. Auth announces sign-ins, sign-outs and password changes.

### Fixed

- An island that is itself the swapped-in element now starts, and a success
  message raised with `lat.flash` leaves after five seconds.

## [0.9.0] - 2026-09-29

Accounts and forms: operators are created, reset and unlocked from the panel
under a password policy; forms fill the width, lay fields out on a grid and rule
what Enter does; a type's keys are written on the entry itself.

### Changed

- **Breaking:** `AuthConfig` is non-exhaustive and gains `password_policy`;
  build it from `Default` and set fields.
- **Breaking:** `AccessEvent` is non-exhaustive and gains `PasswordChanged`,
  recorded when an operator changes their own password.
- **Breaking:** `AuditEntry` and `AuditRecord` are non-exhaustive and carry a
  `target_label`; build an entry with `AuditEntry::new(actor, action).target(..)`.
- **Breaking:** `FormField` is non-exhaustive and gains `enter`, `span` and
  `break`; build one with `FormField::of` or from YAML.
- **Breaking:** a type's keys are written on the entry itself, `rows: 8` on a
  textarea or `options: [..]` on a select; the nested `options: { .. }` is refused.
- **Breaking:** `ListColumn` and `ListFilter` are non-exhaustive, and a column
  carries its type's keys in `options`.
- Pages and form cards fill the width beside the sidebar, or the whole window
  without one. The 1200px page and 720px form caps are gone.
- A sign-in lockout counts failures per username and address, so a stranger
  cannot lock an operator out from elsewhere; one address is also limited
  across usernames (`max_failures_per_address`, default 20).

### Added

- `[auth.password_policy] min_length` (default 8) is enforced wherever a
  password is set: Preferences, first-run setup and `lat admin create`.
- `AuthService::recent_access` reads an account's access log.
- An Access Log screen under Settings → System, gated by the new
  `backend.view_access_log`: sign-ins, failures, lockouts and sign-outs.
- Expired sessions and stay-signed-in credentials are purged at boot and
  hourly, and by `lat admin purge`; `AuthService::purge_expired`.
- The audit log names its target: a username or a role name beside the id.
- Operators are created from the Users screen: name, email and roles, with a
  generated temporary password shown once. `AuthService::create_operator`.
- `textarea` takes `rows` and `placeholder`, `text` takes `placeholder`. A key a
  type does not read is refused by name; `FieldType::option_keys` declares them.
- `span` on a form field (`1/2`, `1/3`, `2/3`, `1/4`, `3/4`, `full`, or 1 to 12
  columns) and `break` lay fields out on a twelve-column grid that wraps.
- Keyboard rules for every form: Enter submits, Cmd/Ctrl+Enter submits from a
  textarea, a picker or a repeater row acts within itself. `enter: off` on a
  form and `enter: next` on a field opt out.
- An administrator resets another operator's password from their edit screen
  (temporary, shown once, every device signed out) and unlocks a locked-out
  account. `AuthService::reset_operator_password`, `is_locked_out`, `unlock`.
- A temporary password must be changed at first sign-in: every screen redirects
  to Preferences until it is. `--generate` on `lat admin create` and
  `reset-password` marks it; `AuthService::require_password_change` does too.

### Fixed

- Enter in a record picker's search box submitted the whole form around it.
- `lat i18n extract` reads `self.tf` calls in the pre-auth templates.

## [0.8.2] - 2026-09-28

Password hardening: every sign-out says why, a change is one transaction and
is dated from creation, a refusal redirects back, and creating an operator is
on the audit trail.

### Added

- `SessionHandle::push_flash_sticky` keeps a message until dismissed; a
  password change confirms with one.
- Creating an operator from first-run setup or `lat admin create` is audited
  as `backend.user.create`, credited to the process.

### Fixed

- Signing one device out from Preferences tells that device why, the same as
  signing out every other device does.
- A password change and a deactivation each run as one transaction, and the
  time a password was set is recorded at account creation.
- A refused password change redirects back with its reason, and the login
  screen's forgot-password link, which went nowhere, is now a note.

## [0.8.1] - 2026-09-28

Password changes that hold, and resources as files: a change signs the account
out everywhere else and says why, is audited and dated, and can be made from
Preferences; a resource can be written as one YAML descriptor file.

### Security

- `lat admin reset-password` now signs the account out of every session and
  stay-signed-in device. Before, a reset left whoever else held it signed in.

### Added

- `AuthService::change_password` revokes every other session with a reason, is
  audited, and refuses a password under `MIN_PASSWORD_LENGTH` (8).
- Operators change their own password under Preferences, confirming the current
  one; this browser stays signed in.
- When a password last changed shows on the Users form and in Preferences.
- A signed-out session or stay-signed-in device is told why on the login screen:
  a changed password, deactivation, sign-out elsewhere, or inactivity.
- A resource can be written as a YAML file and read with
  `laterite_admin::descriptor::from_yaml`. One file per resource, columns and
  fields as named maps, an unknown key refused with its line.
- `resource!("descriptors/posts.yaml")` embeds a descriptor file at compile time.
- `lat i18n extract` reads the labels a descriptor file carries, and
  `lat doctor` parses every `descriptors/**/*.yaml` without a compile.
- The built-in audit log is itself a descriptor file.
- `Permission::renamed_from` rewrites a renamed code in every role and
  per-user override at boot, so an operator's own choices are repaired.

### Fixed

- An htmx request from a signed-out session navigates to login through
  `HX-Redirect`, instead of rendering the login page inside the swap target.

## [0.8.0] - 2026-09-22

Roles you can actually use, and lists you can shape: two built-in roles written
from the permission registry, a screen to assign them, and the knobs a resource
needs to say how its list behaves.

### Changed

- **Breaking:** `Permission` is built with `Permission::new(code, label, group)`
  and is non-exhaustive; `.roles([..])` names the built-in roles that hold it.
- **Breaking:** a `ListFilter` carries a `type` string and an options bag, like a
  `FormField`; `FilterKind` is gone. `boolean` and `select` are unchanged.

### Added

- Two built-in roles, Administrator and Editor, written from the permission
  registry at every boot. The first operator holds Administrator.
- Roles are assigned from the Users screen. A role granting more than the
  editor holds, and their own roles, are locked.
- A built-in role is read-only in the editor, with Duplicate to make your own,
  and cannot be deleted: the assignments that cascade with it would not return.
- A list sets its empty-state message with `no_records_message`, and its search
  box with `SearchConfig`: a prompt, ask on Enter only, or off.
- `per_page_options` offers the operator a choice of page size in the list
  setup, remembered with their column choice.
- Column knobs: `sortable`, `invisible`, `width`, `align` and `require`. A column
  the operator may not see is not queried, sorted, searched or exported.
- Filter knobs: `default_value` opens a list already narrowed, `require` gates a
  filter, and the types `text`, `number` and `date` each refuse what they cannot
  match.

### Fixed

- The New button on a resource list pointed under the admin mount twice and
  answered 404.
- A list column naming an unregistered type stops the application at boot,
  naming the column, instead of rendering an empty cell.

## [0.7.0] - 2026-09-18

The admin's list screens rebuilt into one action row, a curated icon set that
fails loudly on a wrong name, export made opt-in, colour mode moved to core for
the public site, third-party notices accounted for, and the guide rewritten as
a reference: every page code first, options in tables, no rationale.

### Changed

- **Breaking:** an unknown icon name now stops the application at boot, naming
  the item and suggesting near matches, instead of rendering a generic glyph.
- **Breaking:** list screens put New, Delete and search in one row above the
  table, and filters sit on the table as its top strip. Custom CSS on
  `.lat-bulkbar`, `.lat-listbar__form` or `--surface-filterbar` needs updating.
- **Breaking:** a list exports only when it calls `.exportable()`, and the route
  is not registered otherwise. Of the built-ins only the audit log keeps it.

### Added

- An icon set of 197 glyphs in `laterite_core::icons`, served as one cached
  sprite. Modules register their own, namespaced. See the icon reference.
- Colour mode (light, dark, follow-the-system) moved to `laterite_core::theme`,
  so a public site can use the same mechanism the admin does.
- Bundled fonts, icons and scripts are recorded in each crate's `NOTICE`, which
  `cargo about` finds when you generate your own.

### Fixed

- Dropdown menus close on choosing a link, on a click elsewhere, and on Escape.
  `data-lat-keep-open` opts a menu out of it.
- Saving a settings screen or a user's permissions returns to that screen
  rather than the index or the user list.

## [0.6.2] - 2026-09-16

### Fixed

- **Auto colour mode did not follow the system.** It resolved the system
  preference once, at page load, and then froze: an admin left open through dusk
  stayed light until someone reloaded it, which is exactly when an operator
  expects it to have followed along. Auto now listens for the preference
  changing and re-applies. An explicit light or dark is the operator overriding
  the system and still stays put. The boot script also moved into one shared
  partial, so the pre-auth screens get the same behaviour instead of their own
  copy of the old one.

### Added

- **Colour mode is a framework mechanism, not admin chrome.** Light, dark or
  follow-the-system now lives in `laterite_core::theme`, so a public site can use
  the same machinery the panel does: `boot_markup()` inlines a blocking script
  that resolves the mode before first paint and keeps `auto` in step with the
  system, and the contract it establishes (`<html data-theme>`, the `lat-mode`
  storage key, `latMode` / `latSetMode` / `latApplyMode`) is public API pinned by
  a test. What is offered is the mechanism, never an appearance: no icons, no
  toggle markup, no colour tokens, since a site brings its own. The admin is now
  one consumer of it and keeps only its own button and glyph.

- **A way out to the site from the admin.** The top bar carries a link to the
  site's own root, beside the colour-mode button, opening in a new tab so an
  operator does not lose the screen they were working on. The target is the
  configured `app.url` where set, else the bind address, so it follows a
  deployment rather than assuming localhost.

- **An operator account can be deactivated and reactivated.** The users list has
  always shown and filtered on an Active column, and the framework refused a
  sign-in from an inactive account, but nothing could set the flag: the stated
  reason operators are not deletable ("deactivating is the reversible
  equivalent") had no implementation behind it. Deactivating now ends the
  account's sessions and drops its stay-signed-in credentials in the same call,
  because an account that may not sign in must not stay signed in somewhere.
  Two deactivations are refused with a reason the operator reads: your own
  account, and the last active superuser, both of which leave a panel that has
  to be repaired from the command line. The change is audited.

## [0.6.1] - 2026-09-16

### Changed

- **A settings description is a label, not a lecture.** It renders in a sidebar
  211 pixels wide, so it is capped at 72 characters (two lines) by a debug
  assertion on the builder. The reference system's own run 27 to 90 characters,
  median 53; the discovery plugin's ran 80 to 130, every one longer than that
  maximum and three to four lines deep in a narrow column. `SettingsItem::hint`
  is the new home for anything longer: a note at the top of the screen itself,
  uncapped, since the reader has chosen to open it. It supersedes the
  description there, so the two never print together.

## [0.6.0] - 2026-09-16

### Changed

- **Breaking:** a settings screen mounts in its module's namespace
  (`/admin/rainmill/discovery/robots`) rather than at its storage key
  (`/admin/settings/rainmill.discovery.robots`). `SettingsItem.code` is a storage
  key and was being used as a URL, which put a dotted identifier in the path and
  left these the only contributed screens a deployment could not move with
  `[backend.paths]`. They now resolve through the same module namespace as
  resources, screens and public routes, and claim their path in the same
  collision check, so a settings screen shadowing a resource aborts the boot
  instead of silently winning. `SettingsItem` is `#[non_exhaustive]` with a
  `new` + builder; construct it with `SettingsItem::new(code, label, fields)`
  and the builder methods rather than a struct literal. The panel's own settings
  belong to no vendor, so they pin a plain path under `/settings`
  (`/settings/branding`) rather than carrying `laterite` through a URL.

- **Breaking:** the session functions on `laterite_auth::store` (`insert_session`,
  `find_valid_session`, `renew_session`, `delete_session`, `set_session_data`) are
  crate-visible. Reading a session row is what pushes its idle clock forward, so a
  caller reaching the row directly would authenticate a request while leaving the
  session ageing as though it never happened. `AuthService` is now the only way
  in, which makes skipping the renewal unrepresentable rather than merely
  discouraged. The rest of `store` (users, roles, preferences, audit) is
  unchanged. Callers outside the crate were already using `AuthService`; the
  identically named methods on it are the replacement.

### Fixed

- **A form using a reference field could not boot.** Assembling the field-type
  registry built it twice: once with the reference field inserted (it needs the
  picker registry, so it cannot come from the arg-free built-ins) and again from
  the built-ins alone, and the second discarded the first. Any form declaring a
  reference field then aborted boot with "uses unregistered type `reference`".
  Module-contributed types now join the existing registry instead of replacing it.

- **"Stay signed in" did nothing.** The login form offered the box and no code
  read it: the session cookie carried no lifetime at all, so it expired with the
  browser while the session behind it stayed valid for its full twelve hours,
  unused. Ticking the box now gives the cookie that same lifetime, and leaving it
  unticked keeps the browser-session cookie that suits a shared machine.

- Admin forms were pinned to a hardcoded width in five templates, and to two
  different widths between them. They share one token now, and a form holding a
  repeater uses the page's full width rather than cramming a row of columns into
  a column's worth of space. Repeater labels align at the top of a row, so a
  short input and a tall textarea read as one row instead of two.

### Added

- **A repeater can lay its rows out as a list.** A row of several fields rendered
  inline is a row of columns, and four of them is already a table nobody can
  read. `FormField::repeater_list` collapses each row to the line that names it
  and opens one at a time, so Add reads as adding to a list rather than growing
  the form. The naming field is nominated by the descriptor or defaults to the
  first, the title tracks what is typed, and the collapse is a `details` element
  so it works before any script runs. The inline layout stays the default, since
  a single narrow column is better left expanded.

- A dotted segment in an admin path aborts the boot, naming the contribution.
  A module identity is dotted and a URL is not, so an identifier reaching the
  router unresolved is now caught for every contribution type rather than found
  by eye months later. Public routes are exempt, since one may legitimately name
  a file.

- A repeater's sub-fields render their help text, which the descriptor has always
  accepted and the template silently dropped.

- **Responses say how long they may be reused, and prove it cheaply.** Built-in
  assets are now addressed by a digest of their bytes (`laterite.d10210ad.css`),
  and the cache policy follows the URL rather than being declared per asset: a URL
  that names its content may be kept forever, one that does not must revalidate
  and carries an `ETag` so revalidation is a bodyless `304`. A framework upgrade
  therefore reaches a browser that cached the previous build without anyone
  remembering to say so. Admin screens are `private, no-store`, being
  per-operator and carrying a request token, and every admin response carries
  `Vary: HX-Request`, since an htmx fragment and a full page share a URL.
  `laterite_admin::http_cache` exposes the conditional-response helper to plugin
  routes. None of this is disabled in development: a validator that only appears
  in production is one nobody has tested.

- **Sign-ins record where they came from.** `backend_access_log` has carried
  `ip_address` and `user_agent` columns since it was added and wrote null to both:
  `RequestContext` was built with `::default()` at every call site outside tests, so
  the trail recorded who and when but never from where. One layer now resolves both
  per request, and sessions carry them too, so the sessions list names a device
  ("Chrome on macOS") and the address beside it. `X-Forwarded-For` is believed only
  from peers inside the new `backend.trusted_proxies` CIDR ranges, taking the
  rightmost address that is not itself a trusted proxy; empty (the default) trusts
  nothing and records the peer. User agents are stored capped at 512 bytes, since
  the header is caller-controlled and unbounded. Deployments behind a load balancer
  should set `trusted_proxies`, or every row records the balancer.

- **An account can see and end its own sessions.** Preferences now lists every
  browser signed in to the account, newest activity first, marking the one asking
  and showing when each signed in, was last active, and expires. Any other session
  can be ended individually, or all of them at once, which also drops every
  stay-signed-in credential so a signed-out device cannot mint itself a new session
  on its next request. Rows are named by an id derived from the stored key rather
  than the key itself, so a leaked page grants nothing, and a revoke is scoped to
  the asking account. No device or location column yet: those values are not
  captured anywhere yet.

- **"Stay signed in" is a real credential.** Ticking the box previously stretched
  the session cookie's lifetime, which meant the only way to stay signed in for
  a fortnight was a session that lived a fortnight. It now issues a separate
  credential, one row per device, that outlives the session and mints a new one
  when the old expires, so the session itself stays short. The cookie holds a
  public selector and a secret, only the secret's hash is stored, and each use
  rotates the secret. A copy of the cookie therefore works only until the real
  browser next uses it; presented afterwards, the stale secret is taken as proof
  the cookie is in two places and every credential for that account is dropped.
  Signing out revokes the credential rather than leaving it to sign the next
  request straight back in. `remember_duration_secs` sets the window (14 days).

- **Sessions slide.** A session ran on one clock, fixed at login, so an operator
  mid-task was signed out on the same schedule as an abandoned tab. There are two
  clocks now: `session_idle_timeout_secs` (2h) runs from the last request and
  moves forward as work happens, and `session_absolute_timeout_secs` (12h, the
  former `session_ttl_secs`, still read) runs from login and does not move, so a
  stolen token cannot be kept alive indefinitely. Whichever falls first ends the
  session. Renewal happens once past the halfway mark of the idle window rather
  than on every request, which turns a write per request into roughly one an hour.
  Deployments that set no idle timeout gain one: a session left quiet for two
  hours now ends where it previously ran the full twelve.

- A module can contribute a **field type**. The registry held the framework's own
  types only, so a custom form input meant changing `laterite-admin` itself,
  while the sibling column-type registry had been open to modules since lists
  shipped. `FieldTypeReg` and `add_field_type` close that asymmetry; the key is
  the type's own `view_key`, and a collision aborts boot rather than silently
  replacing a built-in.
- `VerifiedUpload`: a file upload whose request token is checked while the form
  is parsed, rather than before. The token travels as the form's first field, as
  it does for any other form, so a scriptless upload needs nothing special. A
  handler that takes the upload with a bare parser instead has its **response
  refused**: the guard marks the check outstanding and enforces it afterwards, so
  skipping it is a broken route rather than an unguarded one.
- `laterite-media`, the storage layer: a `StorageDriver` trait, a local-filesystem
  disk, and a streaming ingest that hashes a file with BLAKE3 as it arrives. A
  file is never held in memory, the size limit stops the read rather than judging
  it afterwards, and identical bytes on one disk are stored once. The content type
  is sniffed from the leading bytes and never taken from the sender. A blob is
  written to a temporary file and renamed into place, so a crashed upload leaves a
  stray temp file rather than a truncated blob at a name that claims to be its own
  hash. Opt-in: an application that stores no files does not compile it.
- The media record: a file's stable identity pointing at a blob, so replacing a
  file writes new bytes under a new hash while the record's id and every
  reference to it stay put. One blob can back several records (the same bytes
  uploaded twice are one blob and two files), and only the last record holding a
  hash releases the blob for collection. Dedup is scoped to a disk, since
  identical bytes on a local disk and in a bucket are separate copies.
- `laterite_media::module()`: media registers on `Bootstrap` the way a plugin
  does, contributing its migrations through the registry, so `laterite-admin`
  never depends on it and gains no feature flag for it.

### Fixed

- A file upload to an admin route could never succeed. The authenticated guard
  buffered every state-changing body to the 1 MiB form limit to read the CSRF
  token from it, so anything larger was truncated to nothing and rejected as a
  CSRF failure, naming the wrong problem. A `multipart/form-data` body now passes
  through unbuffered, and its token comes from the request header or, with
  scripting off, the form action's query string. The origin check runs first on
  every state-changing request either way.

### Changed

- **Breaking:** `Resource`, `ListConfig` and `FormConfig` are `#[non_exhaustive]`
  and are built with `::new` plus builder methods. A struct literal outside the
  crate no longer compiles, `..Default::default()` included, because a
  non-exhaustive struct admits no struct expression at all. Without this their
  field sets would freeze at 1.0 and no descriptor could ever gain a field; the
  next one is the bulk-import descriptor. `ListConfig::creatable()` is opt-in,
  matching the documented read-only default.

### Fixed

- A repeater's sub-field labels rendered untranslated. They are declared inside
  the field type and never pass through the caller that localizes an ordinary
  label, so they could only render from their source string: English on every
  screen, whatever the operator's locale. `FieldCx` now carries the request's
  translator, which is what a composite field type needs to localize strings of
  its own.

## [0.5.1] - 2026-09-15

A security fix for list export, plus the plugin install flow: a plugin is
installed by pointing at it, and one built for another Laterite is refused before
it is fetched.

### Security

- **List export wrote spreadsheet formulas verbatim.** A record whose text began
  `=`, `+`, `@`, a tab or a carriage return was written to CSV unchanged, so a
  value a visitor supplied could execute when an operator opened the export in
  Excel, Sheets or LibreOffice. Such cells now carry the leading apostrophe those
  applications read as "this is text". A leading `-` is escaped only when the cell
  is not a number, so negative numbers still export as numbers.

### Added

- `lat plugin add <path|git-url>` and `lat plugin remove <name>`: a plugin is
  installed by pointing at it, from a local checkout or a repository, rather than
  by placing a folder in a layout by hand. A local plugin outside the project is
  linked into `plugins/` so it stays where it is and edits apply in place.
- `plugins/plugins.toml`: the declarative list of the plugins an application
  compiles in, maintained by those two commands. **Folder names no longer carry
  meaning**, so nothing has to be renamed to be installed: the list says where a
  plugin is and its own `Cargo.toml` says what it is called, which also means the
  two can no longer disagree. An application using the previous
  `plugins/<author>/<plugin>/` layout adopts it into a list on first use.
- `lat new` scaffolds the plugin layout: the workspace, an empty list, the
  generated manifest, the dependency on it, and the `.modules(...)` call.
- `[package.metadata.laterite]` in a plugin's manifest, declaring the module it
  registers. Its presence marks the crate as a plugin, and `lat plugin add` now
  refuses one without it instead of letting the build fail on a missing
  `module()`. Readable without building, so a marketplace can list and filter on
  it. It carries no version field: which Laterite a plugin supports is already
  stated by its dependency requirement.
- `lat plugin add` checks that requirement against this application's own version
  before fetching or building, and refuses a plugin built for another Laterite by
  name. Boot stays the backstop, as it does for database capabilities.
- Guide: Installing Plugins.

### Fixed

- `lat plugin sync` reported success while writing a manifest nothing compiled.
  A `lat new` application was never wired for plugins at all, so a plugin added
  to one silently did nothing; sync now warns when the application does not
  depend on the generated manifest, and a new application no longer needs the
  wiring done by hand.
- Installing one plugin twice produced a manifest with a duplicate dependency key
  that cargo refused to parse. It is now refused when added, naming where the
  plugin already is, and nothing is left behind by the refusal.

## [0.5.0] - 2026-09-12

Interactive admin and Lists complete: the admin engine's list screens feature
complete, and its forms and lists answering in place rather than reloading.

### Added

- `laterite_admin::axum`: the axum a contributed route builds against, re-exported
  so a plugin never declares axum itself. Two axum majors linked side by side make
  `Router` a different type from `Router`; taking it from here means the
  framework's manifest is the only place the version is chosen.
- `RouteCtx::builder(db)`: a route context built by hand, so a plugin can mount and
  exercise its own `Screen` or `PublicRoute` as an ordinary tower service instead of
  booting the framework to test a route. It is also how the context grows after 1.0.
- Descriptor forms submit through HTMX. A failed save re-renders the form in
  place with its per-field errors instead of reloading the page, and a
  successful one redirects through the `HX-Redirect` header. A browser with
  scripting off still gets the whole page and an ordinary redirect.
- The generic form flashes on success, which only the roles form did before.
- The role editor submits through HTMX on the same contract, so a rejected code
  or name re-renders the permission editor in place with the ticks intact.
- Request feedback across the admin: a top progress bar while a request is in
  flight, a disabled submit button for the round trip, and a toast when a request
  fails, which htmx would otherwise discard in silence.
- `window.lat.flash(text, level)` raises a toast from a screen's own script.
- Sortable list columns. Every header sorts, a second click on the sorted column
  reverses it, and only a column the descriptor declares can be ordered by.
- Lists sort and page through HTMX, swapping the table region and pushing the
  URL, so back, refresh and a copied link all land on the same view. Without
  scripting they stay ordinary links.
- `laterite_core::search`: a `SearchProfile` composing text folds
  (`Normalizer`), query expanders, and a `Matcher` that builds the SQL condition.
  Folding runs in Rust so it behaves identically on all three databases. Guide at
  `docs/src/extend/search.md`.
- `TableSource::with_search` sets a picker source's search behaviour.
- Settings screens render and save through the field-type registry, so every
  registered type works there and a module's own type works on both surfaces.
- `FormField` gained constructors for the types that had none: `select`, `radio`,
  `switch`, `date`, `password` and `repeater`. A descriptor previously reached
  them through `of` plus a raw options blob.
- A `repeater` field type: a list of rows stored as a JSON array of objects.
  Row controls are named `field[index][subfield]`, and the indices are read back
  out of the submitted keys and sorted, so a row removed in the browser leaves no
  hole. A row's sub-fields are ordinary field types, so a switch inside a row
  stores a bool and a date is a date. `min_items` and `max_items` bound the list.
- A module can read another module's contributions. It defines a type, others
  contribute it from `register`, and its routes read them with
  `RouteCtx::contributions`. The framework never learns the type, and everything
  is collected before any route mounts, so registration order does not matter.
  This is what `Module::register` already promised by "plugin-defined extension
  items".
- `RouteCtx::base_url` reports the site's own origin, for the absolute URLs a
  route emits rather than links (a sitemap `<loc>`, a canonical URL, a feed).
- The column-type registry is open to modules. A module contributes a
  `ColumnTypeReg` and its type renders those cells; the key is the type's own
  `view_key`, and a duplicate aborts the boot naming it.
- `StaticSite::file` writes a file verbatim at the output root, for the files a
  site decides for itself (`robots.txt`, `sitemap.xml`, `humans.txt`, `llms.txt`,
  a `CNAME`, a verification token). It creates parent directories and refuses a
  path that climbs out of the output directory.
- `StaticSite::paths` and `StaticSite::base_url` report every page written and
  the prefix to form absolute URLs from, which is what a sitemap is built from.
- List export. Every list offers CSV and JSON from the toolbar, carrying the
  current columns, search, filters and sort, so the file matches the screen. An
  export beyond 20,000 rows is refused rather than truncated.
- Toolbar buttons are descriptor data. A resource declares its own with
  `ToolbarButton`, optionally gated by a permission and carrying an icon, and the
  framework contributes New and the export menu the same way.
- `ListConfig` implements `Default`, so a descriptor sets what it means and ends
  with `..Default::default()`. Fields added to it after this are additive.
- Per-operator column configuration. A list offers a Columns disclosure, and the
  choice is stored against the account and applied on every visit. It narrows the
  descriptor once per request, so the visible columns are what gets queried,
  sorted and searched. A stale choice falls back to every column rather than
  leaving an empty table.
- `backend_user_preferences`, a per-operator key/value store, with
  `store::user_preference`, `set_user_preference` and `clear_user_preference`.
  The settled preferences every account has (locale, timezone) stay as columns on
  `backend_users`; this is for per-screen choices that arrive one screen at a time.
- Deleting joins the record layer. `ModelListener::before_delete` sees the row
  inside the transaction and can refuse the delete with a message the operator
  reads; `after_delete` runs once it has committed. `Persister::delete` performs
  it, and refuses by default so a persister that has not implemented deletion
  says so rather than appearing to succeed. Deletes are audited by the same
  listener that records every other write.
- Bulk delete on a list. A list marked `deletable` carries a checkbox per row and
  a select-all header, and the Delete button asks for confirmation first. Each
  record goes through the delete pipeline in its own transaction, so one refusal
  reports its reason and leaves the rest of the selection deleted. The roles list
  is deletable; the users list and the audit log are not.
- A confirm dialog for any control carrying `data-lat-confirm`: the click is held
  in the capture phase until the operator agrees, and Escape or Cancel drops it.
- Empty list states read correctly: a table with nothing in it offers a New link,
  while a search or filter that matches nothing says so instead of reporting the
  table as empty.
- List filters. A descriptor declares a filter per column (`ListFilter::boolean`,
  or `select` with a fixed option set) and the framework renders the controls,
  applies them to the rows and the count, and keeps them across a sort or a page.
  Only a declared filter, and for a select only a declared option, reaches the
  query. The backend users list filters on Active and Superuser.
- List search. A box in the toolbar filters as you type, across the columns the
  descriptor marks searchable (text columns by default; `ListColumn::searchable`
  overrides). It sits outside the swapped region so it keeps the caret, the term
  survives a sort or a page, and a blank term is no filter. With scripting off it
  is a GET form and Enter searches.

### Removed

- **Breaking**: `laterite-web` no longer writes a `robots.txt` or a
  `sitemap.xml`, and `StaticSite::finish` is gone with them. How a site wants to
  be crawled, and which URLs it advertises at what priority, are the
  application's decisions. `StaticSite::paths` and `base_url` give the material
  and `file` publishes the result, so a site keeps both files by writing them.

### Changed

- **Breaking:** `ScreenReg` and `PublicRouteReg` are `#[non_exhaustive]`. Both are
  built with `::new` plus builder methods already, so nothing in tree changes; a
  struct literal in an out-of-tree plugin no longer compiles. Without this their
  field set would be frozen at 1.0 and the framework could never learn anything new
  about a contributed route.
- A refused save answers 422 rather than 200, on both the descriptor form and
  the role editor.
- **Breaking**: `SettingsWidget` and `SettingsField` are gone. A settings model
  declares `FormField`s, the same descriptors a form screen uses, and its widget
  is its field type. There is no settings-specific field vocabulary.
- **Breaking**: `FieldType::to_attr` takes a `SubmittedField` rather than one
  value, so a field made of several controls can read its own keys. A scalar type
  calls `field.value()` for what it had before. This is what kept multi-value
  fields (a repeater, a checkbox list) out of the field system.
- **Breaking**: `FieldType::resolve_options` takes the field registry, so a
  composite field can resolve the types of the fields it holds while the registry
  is in hand.
- **Breaking**: a registry contribution must be `Send + Sync`, since the ones the
  framework does not consume stay readable from request handlers. Every existing
  contribution type already was.
- **Breaking**: `router` takes one `Contributions` value rather than nine
  positional vectors, including the new column types. Build it with the fields you
  mean and end with `..Default::default()`; a later kind of contribution is then a
  new field rather than a new argument. Applications boot through `Bootstrap` and
  are unaffected.
- **Breaking**: `ListConfig` gained `filters`, `deletable` and `toolbar` fields. A descriptor now ends with
  `..Default::default()` and sets only what it means, which is also why this is
  the last such break.
- The picker matches through a `SearchProfile` rather than its own `LIKE`.
  Behaviour is unchanged: case folding, literal wildcards, and an empty query
  still lists the first rows.

## [0.4.0] - 2026-09-09

Three steps: the record layer (writes run through one transaction with lifecycle
listeners and a named actor), plugin screens and routing (a module mounts its own
admin screens and public routes, and every contributed path is resolved and
checked for collisions), and the typed form contract (a field type decides what it
stores and shows). Plus the CLI and licensing work that had been waiting.

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
  screen reads a narrow `RouteCtx` and builds self-links through `ctx.url`, so a
  moved screen moves its links with it.
- A module can mount public routes at literal paths, outside the admin, for
  robots files, sitemaps, feeds and webhook receivers. A public route that reaches
  inside the admin mount aborts the boot.
- `RouteCtx::admin_path` reports where the panel actually mounted, so a module
  links into it or excludes it without assuming `/admin`.
- A screen that declares `in_menu` appears in the main menu; one that does not
  mounts silently, reached from a link elsewhere.
- The Screens and Routes guide.
- A field type decides what its value stores and how it comes back:
  `FieldType::to_attr` turns a submission into a typed attribute (or omits it, so
  a blank password on an edit leaves the stored one alone), and
  `FieldType::to_control` turns a stored value back into what the control shows.
- A `switch` field type over a boolean column. An unchecked box submits nothing,
  so it stores `false` rather than leaving the previous value.
- A `date` field type. It stores the date a control sends and trims a stored
  timestamp back to its date when presenting, since a date input rejects a full
  instant. Carries a `Date` validation rule.
- A `password` field type: hashed with Argon2 on save, never rendered back into
  the page, and a blank submission on an edit leaves the stored password alone.
- A `radio` field type over the same options as `select`, so a descriptor swaps
  between them by changing the type alone.
- `Rule::Date`, and `Rule` is now non-exhaustive so later rules are additive.
- A `timestamps` flag on a form stamps `created_at` and `updated_at`, leaving a
  supplied `created_at` alone so an import keeps its history.
- The Model Listeners guide.
- **Breaking**: `router` takes the model-listener contributions. Applications
  boot through `Bootstrap` and are unaffected.
- The built-in persister binds typed values (integers, booleans as integers,
  timestamps and JSON as text) and writes attributes a listener added, not only
  the descriptor's own columns.

### Fixed

- A request below a public route (`/robots.txt/anything`) escaped into axum's
  bodyless 404 instead of the styled error page, because a nested service does not
  inherit the outer fallback.
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
