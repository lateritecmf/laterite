# Changelog

Notable changes to the Laterite crates. The crates share one version and release
together. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/) as Cargo reads it: before
1.0 a minor bump is breaking and a patch is additive.

## [Unreleased]

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
