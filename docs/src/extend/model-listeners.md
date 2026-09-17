# Model Listeners

A listener runs when a record is written: fill a column, refuse a save, react
to a change. It reaches every entity written through the save pipeline.

## The record

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

fn register(&self, registry: &mut Registry) {
    registry.add(ModelListenerReg::for_entity("articles", Arc::new(Slugify)));
    // ModelListenerReg::all(..) targets every entity
}
```

Hook | Runs | Return
--- | --- | ---
`before_save` | Inside the transaction | `Err(ErrorBag)` refuses and rolls back; the operator sees the messages on their fields.
`after_save` | After commit | Side effects. Cannot roll back.
`before_delete` | Inside the transaction | `Err(Text)` refuses and rolls back.
`after_delete` | After commit | Side effects.

All optional. Keep `before_save` fast: the transaction stays open across it.

## Read what changed

```rust
if rec.has_original() && rec.changed("title") {
    let was = rec.original("title").and_then(AttrValue::as_str);
}
```

`changed` is false without a snapshot; check `has_original` first. The
comparison is text-level.

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

Sets `created_at` on create and `updated_at` on every write; a `created_at`
already on the record is kept. Opt in per form: a column added to every entity
reaches tables without it.

## Guard a delete

```rust
#[laterite_core::strata::async_trait]
impl ModelListener for Guard {
    async fn before_delete(&self, cx: &mut SaveCx<'_>, rec: &Record) -> Result<(), Text> {
        Err(laterite_core::t!("Three records still reference this one."))
    }
}
```

Deletion is not an `Op`: a save listener never receives deletes. A `Persister`
refuses deletion unless it overrides `delete`.
