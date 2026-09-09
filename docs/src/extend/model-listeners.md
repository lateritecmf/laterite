# Model Listeners

A listener runs when a record is written. Use one to fill a column, refuse a save,
or react to a change, without editing the screen that performs the write.

Listeners reach every entity written through the admin's save pipeline: your
resources, another module's, and later content types defined at runtime.

## The record

A write is a `Record`: an entity name, an optional id, and typed attributes.

```rust
let mut rec = Record::new("articles");
rec.set("title", "Hello")
   .set("views", 0i64)
   .set("published", true)
   .set("published_at", Utc::now());

rec.text("title");        // Some("Hello")
rec.int("views");         // Some(0)
rec.deserialize::<Article>()?;   // a struct view, ignoring extra attributes
```

An absent attribute and a null one differ: absent means "not part of this write",
null means "write NULL".

## Writing a listener

```rust
use laterite_core::{ErrorBag, ModelListener, Op, Record, SaveCx, SavedCx, t};

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
            return Err(bag);   // refuses the write; the operator sees this
        }
        rec.set("slug", slugify(title));
        Ok(())
    }
}
```

Register it from your module, for one entity or for all of them:

```rust
fn register(&self, registry: &mut Registry) {
    registry.add(ModelListenerReg::for_entity("articles", Arc::new(Slugify)));
}
```

## The two stages

`before_save` runs **inside the transaction**, so a read there sees the in-flight
write. Change the record, or return an `ErrorBag` to refuse the save: the whole
transaction rolls back and the operator sees your messages against their fields.

`after_save` runs **after the commit**, for side effects. It cannot roll the write
back, and its own failure leaves the row saved. The framework audits writes here.

Because the transaction stays open across `before_save`, do your work and return.
Do not call a slow external service from it.

## Knowing what changed

On an update the record may carry the row as it stood before the write:

```rust
if rec.has_original() && rec.changed("title") {
    let was = rec.original("title").and_then(AttrValue::as_str);
}
```

`changed` is always false without a snapshot, so check `has_original` first rather
than reading "unchanged" into its absence. The comparison is text-level: a listener
that re-types an unchanged value reads as changed.

## Who is writing

Every save states an actor, and both stages can read it:

```rust
match cx.actor() {
    Actor::User { id, username } => rec.set("updated_by", *id),
    Actor::System { process } => { /* a seeder, a job, the CLI */ }
    _ => {}
}
```

A write with no person behind it names its process rather than borrowing a user, so
the audit trail stays honest.

## Timestamps

The framework ships a timestamps listener. Opt in per form, because the columns
have to exist on that table:

```rust
FormConfig { timestamps: true, .. }
```

It sets `created_at` on create and `updated_at` on every write, and leaves a
`created_at` already on the record alone so an import can preserve history.

Do not register a column-stamping listener for every entity. The built-in
persister writes whatever the record holds, so an attribute added everywhere
reaches tables that have no such column and fails their writes.

## Deletion

Deleting runs the same shape as saving: the pipeline owns the transaction, and a
listener sees the row before it goes.

```rust
# use laterite_core::{ModelListener, Record, SaveCx, Text};
# struct Guard;
#[laterite_core::strata::async_trait]
impl ModelListener for Guard {
    async fn before_delete(&self, cx: &mut SaveCx<'_>, rec: &Record) -> Result<(), Text> {
        // Read through cx.conn(), inside the same transaction as the delete.
        Err(laterite_core::t!("Three records still reference this one."))
    }
}
```

Returning a message refuses the delete and rolls it back, and the operator sees
that message. This is where a module guards records that point at this one,
rather than discovering the dangling row later.

`after_delete` runs once the delete has committed, for side effects such as
removing files the row owned. Like `after_save` it is not atomic with the write:
the row is already gone, so failing there cannot bring it back.

Deletion is deliberately not an `Op` variant. A listener written for saves must
not silently start receiving deletes: `Timestamps` stamps `updated_at` on every
save, which is nonsense for a row being removed. Separate methods mean a listener
opts into deletion.

A `Persister` refuses deletion by default. One that writes across more than one
table has to say how those rows come apart, and guessing at that is worse than
declining, so an implementor opts in by overriding `delete`.
