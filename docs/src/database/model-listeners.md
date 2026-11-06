# Model Listeners

A listener runs when a record is written: fill a column, refuse a save, react
to a change. It reaches every entity written through the save pipeline.

## Build a record

```rust
let mut rec = Record::new("articles");
rec.set("title", "Hello")
   .set("views", 0i64)
   .set("published", true)
   .set("published_at", Utc::now());

rec.text("title");               // Some("Hello")
rec.int("views");                // Some(0)
rec.deserialize::<Article>()?;   // a struct view; extra attributes ignored
```

An absent attribute is not part of the write; a null one writes `NULL`.

## Write a listener

```rust
use laterite_core::{ErrorBag, ModelListener, Op, Record, SaveCx, t};

struct Slugify;

#[laterite_core::strata::async_trait]
impl ModelListener for Slugify {
    async fn before_save(
        &self,
        cx: &mut SaveCx<'_>,
        rec: &mut Record,
        op: Op,
    ) -> Result<(), ErrorBag> {
        let Some(title) = rec.text("title") else {
            return Ok(());
        };
        if title.is_empty() {
            let mut bag = ErrorBag::default();
            bag.add("title", t!("A title is required."));
            return Err(bag);
        }
        rec.set("slug", slugify(title));
        Ok(())
    }
}
```

Hook | Runs | Return
--- | --- | ---
`before_save` | Inside the transaction | `Err(ErrorBag)` refuses and rolls back; the user sees the messages on their fields.
`after_save` | After commit | Side effects. Cannot roll back.
`before_delete` | Inside the transaction | `Err(Text)` refuses and rolls back.
`after_delete` | After commit | Side effects.

All optional.

## Register it

```rust
fn register(&self, registry: &mut Registry) {
    registry.add(ModelListenerReg::for_entity("articles", Arc::new(Slugify)));
    registry.add(ModelListenerReg::all(Arc::new(AuditWriter)));
}
```

Registration | Targets
--- | ---
`ModelListenerReg::for_entity(entity, listener)` | Records of one entity.
`ModelListenerReg::all(listener)` | Every entity. Must not assume a column exists.

## Read what changed

```rust
if rec.has_original() && rec.changed("title") {
    let was = rec.original("title").and_then(AttrValue::as_str);
}
```

`changed` is false without a snapshot; check `has_original` first.

## Read the actor

```rust
match cx.actor() {
    Actor::User { id, username } => rec.set("updated_by", *id),
    Actor::System { process } => { /* a seeder, a job, the CLI */ }
    _ => {}
}
```

## Stamp timestamps

```rust
FormConfig { timestamps: true, .. }
```

```yaml
form:
  timestamps: true
```

Sets `created_at` on create and `updated_at` on every write; a `created_at`
already on the record is kept. Opt in per form: a column added to every entity
reaches tables without it.

## Record who changed a row

```yaml
form:
  footprints: true
```

```rust
FormConfig::new("posts", t!("Post"), "/admin/posts", "id", fields).footprints()
```

Column | Set on | Value
--- | --- | ---
`created_user_id` | Create | The signed-in user's id; null for a system write
`updated_user_id` | Every write | The signed-in user's id; null for a system write

A value already on the record is kept.

## Generate a slug

```yaml
form:
  slugs:
    slug: title
    handle: [first_name, last_name]
```

```rust
form.slug("slug", &["title"])
```

Case | Result
--- | ---
The slug is empty | Built from the sources: `Hello World` becomes `hello-world`, then `hello-world-2`, `hello-world-3`
The slug was typed and is free | Stored slugged: `Hello There` becomes `hello-there`
The slug was typed and another row holds it | Refused on the slug field
An edit keeps the stored slug | Stored unchanged

A slug is at most 180 characters, suffix included. Trashed rows hold their slug.

## Seal a column

```yaml
form:
  encrypt: [api_key]
```

```rust
form.encrypt(&["api_key"]);
let plain = rec.opened(&crypt, "api_key");
```

Value written | Result
--- | ---
Plaintext, through the form or an import | Sealed under the application key
Null or empty | Kept
Already sealed, such as from a `sensitive` field | Kept

`Record::opened` returns the plaintext, or `None` when the value does not open.

## Guard a delete

```rust
#[laterite_core::strata::async_trait]
impl ModelListener for Guard {
    async fn before_delete(&self, cx: &mut SaveCx<'_>, rec: &Record) -> Result<(), Text> {
        Err(laterite_core::t!("Three records still reference this one."))
    }
}
```

Deletion is not an `Op`: a save listener never receives deletes. A
[persister](persisters.md) refuses deletion unless it overrides `delete`.
