# Command Line

`lat` scaffolds, runs and maintains an application. Every command accepts
`--database-url <URL>`, falling back to `DATABASE_URL`, then to the
configuration of the application found from the current directory upward.

```bash
cargo install laterite-cli
lat --help
lat <command> --help
```

## Scaffold

Command | Description | Options | Example
--- | --- | --- | ---
`new [NAME]` | Scaffold and set up a new application, or a static website (interactive). | `--static`: a theme folder with no database. `--framework-path <PATH>`: build against a local framework checkout, also read from `LATERITE_FRAMEWORK_PATH`. | `lat new "Acme Blog"`
`make:migration <DESCRIPTION>` | Scaffold a migration file in this crate's `src/migrations/`. | `--path <PATH>`: the crate directory, default `.`. | `lat make:migration create_articles`
`make:plugin <ID>` | Scaffold a plugin crate; `<ID>` is `vendor.name`. | `--path <PATH>`: where to create it, default `.`. `--admin`: scaffold the admin half. | `lat make:plugin acme.blog --admin`
`make:resource <NAME>` | Scaffold an entity's migration, schema, store and admin screen. | `--path <PATH>`: the crate, default `.`. `--field <name:type>`: columns beyond id and timestamps; types `text`, `integer`, `boolean`, `datetime`; repeat or comma-separate. | `lat make:resource article --field title:text,body:text`

## Run and check

Command | Description | Options | Example
--- | --- | --- | ---
`serve` | Run this application, or preview a static website's theme folder, optionally overriding the address. | `--host <HOST>`, `--port <PORT>`, `--listen <HOST:PORT>`, `--release`: build and run optimised. | `lat serve --port 3000`
`doctor` | Check that this application is set up to run. | None. | `lat doctor`

## Local domains

Command | Description | Options | Example
--- | --- | --- | ---
`domain setup` | Route `*.<tld>` to 127.0.0.1 via dnsmasq and a macOS resolver entry. | `--tld <TLD>`, default `test`. `--dry-run`. | `lat domain setup --dry-run`
`domain status` | Report the current local-domain DNS state. | `--tld <TLD>`. | `lat domain status`
`domain teardown` | Undo the wildcard DNS configuration. | `--tld <TLD>`. `--dry-run`. | `lat domain teardown`

## Admin users

Command | Description | Options | Example
--- | --- | --- | ---
`admin create <USERNAME>` | Create a superuser. | `--email <EMAIL>` and `--first-name <NAME>` required; `--last-name <NAME>`; `--password <PASSWORD>`, else prompted; `--generate`: print a strong random password. | `lat admin create editor --email editor@acme.test --first-name Editor`
`admin reset-password <USERNAME>` | Set a new password for a user. | `--password <PASSWORD>`, else prompted; `--generate`. | `lat admin reset-password editor --generate`
`admin list` | List the admin users. | None. | `lat admin list`
`admin unlock <USERNAME>` | Clear a user's failed-login lockout. | None. | `lat admin unlock editor`
`admin purge` | Remove expired sessions and stay-signed-in credentials. | None. | `lat admin purge`

## Plugins

Command | Description | Options | Example
--- | --- | --- | ---
`plugin add <SOURCE>` | Add a plugin from a local path or a git URL, and record it. | `--as <FOLDER>`: the folder under `plugins/`, default the crate's name. | `lat plugin add ../blog-plugin`
`plugin remove <NAME>` | Remove a plugin from the list by crate name or folder. | `--delete`: also delete its folder. | `lat plugin remove acme-blog --delete`
`plugin sync` | Regenerate the `plugins-manifest` crate from `plugins/plugins.toml`. | None. | `lat plugin sync`
`plugin list` | List the plugins this application compiles in. | None. | `lat plugin list`
`plugin check` | Check each installed plugin against the expected layout. | None. | `lat plugin check`

## Translations

Command | Description | Options | Example
--- | --- | --- | ---
`i18n extract` | Regenerate each crate's `lang/messages.pot` from its source strings. | None. | `lat i18n extract`
`i18n check` | Fail if any committed `messages.pot` is out of date. | None. | `lat i18n check`
`i18n update <LOCALE>` | Merge each crate's `messages.pot` into a locale's `.po`, keeping existing translations; creates the `.po` if absent. | None. | `lat i18n update fr`
`i18n status` | Report translation coverage for each crate's locale catalogs. | None. | `lat i18n status`

## Themes

Command | Description | Options | Example
--- | --- | --- | ---
`theme export <CODE>` | Write a theme and its templates to a zip. | `-o, --out <FILE>`, default `<code>.zip`. | `lat theme export acme -o acme.zip`
`theme import <CODE> <FILE>` | Create or update a theme from a zip. | `--replace`: replace templates and config the theme already has; each replaced template keeps a revision. | `lat theme import acme acme.zip --replace`
`theme list` | List the themes. | None. | `lat theme list`
`theme build [CODE]` | Render a theme's fixed pages and assets to a directory, for a static host. | `--from <DIR>`: a theme folder instead of the database, default `theme.folder`. `-o, --out <DIR>`, default `dist`. `--base-url <URL>`, default `app.url`, else one derived from `server.listen`. `--copy <SRC>:<DEST>`: a directory copied into the output; repeatable, added to `theme.copy`. | `lat theme build --copy docs/book:docs`
`theme serve` | Preview a theme folder, rebuilt on every change. | `--from <DIR>`, default `theme.folder`. `--listen <HOST:PORT>`, default `server.listen`. `--copy <SRC>:<DEST>`. | `lat theme serve`

## Media

Command | Description | Options | Example
--- | --- | --- | ---
`media gc` | Remove files outside the library that nothing links to, and the blobs no record names any more. | `--grace <HOURS>`: leave younger files alone, default `24`. `--dry-run`. `--storage <DIR>`, default `backend.storage`. | `lat media gc --dry-run`
`media verify` | Re-hash every stored file against its record. | `--storage <DIR>`, default `backend.storage`. | `lat media verify`

## Key

Command | Does
--- | ---
`lat key generate` | Writes a new application key as `app.key` in `config/local.toml`; `--rotate` keeps the old key under `previous_keys`, `--print` prints without writing.
`lat key show` | The id of the key in use and of the previous keys.
`lat crypt rotate` | Re-seals every `sensitive` column under the key in use, after `lat key generate --rotate`.

## Cache

Command | Does
--- | ---
`lat cache clear` | Moves every cache namespace on; every process reads afresh. `--namespace records::posts` moves one.
`lat cache list` | Every namespace the deployment knows, with its generation.

## Queue

Command | Does
--- | ---
`lat queue work` | Runs the application's worker (`cargo run -- queue work`); `--once`, `--stop-when-empty`, `--queue high,default`, `--release`.
`lat queue list` | How many jobs wait and run on each queue.
`lat queue failed` | The failed list.
`lat queue retry <id\|all>` | Puts a failed job back on its queue.
`lat queue forget <id\|all>` | Drops a failed job.
`lat queue flush [--queue name]` | Drops every waiting job, or one queue's.

## Schedule

Command | Does
--- | ---
`lat schedule run` | Runs the application's ticker (`cargo run -- schedule run`); `--once` evaluates this minute and exits.
`lat schedule list` | Every entry that has run on this deployment, with its last run.

## Mail

Command | Does
--- | ---
`lat mail test <address>` | Sends the built-in test template to the address through `[mail]`.
