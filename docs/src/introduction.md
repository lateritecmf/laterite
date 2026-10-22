# Laterite

Laterite is a content management framework for Rust: an admin panel from
descriptors, themes edited in the admin, a media library, plugins, users and
permissions, migrations and typed settings, on PostgreSQL, MySQL or SQLite.

```bash
cargo install laterite-cli
lat new
```

<div class="lat-cards">
<a class="lat-card" href="getting-started/installation.html"><strong>Install</strong><span>Three commands to a running admin.</span></a>
<a class="lat-card" href="getting-started/directory-structure.html"><strong>Your first project</strong><span>What the scaffold gives you and where things go.</span></a>
<a class="lat-card" href="forms/form-descriptor.html"><strong>Build a screen</strong><span>A list and a form from one YAML file.</span></a>
<a class="lat-card" href="themes/themes.html"><strong>Theme a site</strong><span>Pages, layouts and partials edited in the admin.</span></a>
<a class="lat-card" href="plugins/writing-a-plugin.html"><strong>Add a plugin</strong><span>A folder that brings its screens and tables.</span></a>
<a class="lat-card" href="reference/cli.html"><strong>Reference</strong><span>Every command, behaviour and icon.</span></a>
</div>

Crate | Provides
--- | ---
`laterite-core` | Config, errors, the database layer, migrations, modules.
`laterite-auth` | Backend users, sessions, roles and permissions.
`laterite-admin` | The admin router, list and form screens, settings.
`laterite-media` | Content-addressed file storage. Optional.
`laterite-web` | Static-site generation and page metadata. Optional.
`laterite-cli` | The `lat` command.
`laterite-macros` | The `t!`, `tn!` and `tp!` translation macros.

Section | Covers
--- | ---
Getting Started | Install, run, deploy and upgrade an application.
Configuration | Every key in the configuration files.
Admin Panel | Users, permissions, navigation, branding, settings and logs.
Forms, Lists, Elements | Screens from descriptors, and every field, column and filter type.
Plugins, Screens and Routes | Modules of your own and the screens they mount.
Database, Services, Media | Migrations, queries, events, sessions, search and files.
Themes and Markup | The site's templates and the template language.
Reference | Behaviours, the command line, icons and the API documentation.

Not in the framework today: mail, queues, a scheduler, a cache driver, an
encryption service, multi-site.
