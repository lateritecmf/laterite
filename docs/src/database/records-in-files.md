# Records in Files

A resource's `source` keeps its records as files: one file per record in a
folder of the project, read into the table when the application starts and
written back on every save.

```yaml
# descriptors/posts.yaml
entity: posts
path: /posts
title: Posts
permission: acme.blog.manage_posts
source:
  kind: git
  path: content/posts
  format: markdown
  key: slug
  body: body
  slug_from: title
list:
  creatable: true
  deletable: true
  columns:
    title: { searchable: true }
form:
  title: Post
  fields:
    slug: {}
    title: { rules: [required] }
    pinned: { type: switch }
    body: { type: textarea, rows: 12 }
```

Option | Description
--- | ---
`path` | The folder, relative to the project root. Required.
`kind` | `files` writes the file; `git` also commits it. Default `files`.
`format` | `yaml`, `json`, or `markdown` (YAML front matter and a body). Default `yaml`.
`key` | The text field that names the file. Default `slug`.
`body` | With `markdown`, the field the body under the front matter holds. Optional.
`slug_from` | A field the key is derived from when a save leaves it blank. Optional.

## Scaffold it

```bash
lat make:resource article --field title:text --source content/articles --format markdown --git
```

Flag | Description
--- | ---
`--source <DIR>` | The folder; a `slug` column names each file.
`--format` | `yaml`, `json` or `markdown`; `markdown` adds a `body` column. Default `yaml`.
`--git` | Commit every save.

## Declare it from Rust

```rust
use laterite_admin::Resource;
use laterite_core::source::{Format, Kind, Source};

let posts = Resource::new("/posts", "Posts", list)
    .form(form)
    .permission("acme.blog.manage_posts")
    .source(Source {
        kind: Kind::Git,
        path: "content/posts".into(),
        format: Format::Markdown,
        key: "slug".into(),
        body: Some("body".into()),
        slug_from: Some("title".into()),
    });
```

## Write a file

```markdown
<!-- content/posts/hello.md -->
---
title: Hello
pinned: true
---
# Hello

The first post.
```

Format | File | Holds
--- | --- | ---
`yaml` | `hello.yaml` | The form's fields as a YAML document.
`json` | `hello.json` | The form's fields as a JSON object.
`markdown` | `hello.md` | The fields as front matter, the `body` field under it.

The file's name is the `key` field: letters, digits, dots, dashes and
underscores. The fields are written in the form's order. A switch is `true` or
`false`, a number input is a number, a repeater is a list, anything else is
text.

Starting the application reads the folder: a file's record is created or
updated by key, and a record whose file is gone is removed. A file naming a
field the form does not have fails the start, naming the file and the field.

## Fit the table

```rust
Table::create()
    .table(Alias::new("posts"))
    .col(ColumnDef::new(Alias::new("id")).big_integer().not_null().auto_increment().primary_key())
    .col(ColumnDef::new(Alias::new("slug")).text().not_null())
    .col(ColumnDef::new(Alias::new("title")).text().not_null())
    .col(bool_col(Alias::new("pinned")).null())
    .col(ColumnDef::new(Alias::new("body")).text().null())
    .to_owned()
```

Column | Rule
--- | ---
The `key` field | A text column; the form's `unique` rule keeps one file per key.
A form field | Any column type the field stores.
A column the form does not have | Needs a default, or `null`.

## Commit every save

```yaml
source:
  kind: git
  path: content/posts
```

Action | Commit
--- | ---
Create | `Added content/posts/hello.md (ana)`
Edit | `Updated content/posts/hello.md (ana)`
Rename the key | The old file is removed in the same commit.
Delete | `Removed content/posts/hello.md (ana)`

The repository is the project root; `git` is on the path with a user name and
email configured. Push the branch to publish: a host that builds on push
rebuilds the site.

```bash
git push origin main
```
