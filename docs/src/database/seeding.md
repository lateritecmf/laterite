# Seeding

A seeder puts a module's data in the database: reference rows, defaults,
demo content. It runs when asked, as often as asked.

## Write a seeder

```rust
use laterite_core::seed::{insert_missing, Seeder, SeederSet};
use laterite_core::{CoreResult, Db, Record};

struct Statuses;

#[laterite_core::strata::async_trait]
impl Seeder for Statuses {
    fn name(&self) -> &str {
        "statuses"
    }

    async fn run(&self, db: &Db) -> CoreResult<()> {
        for (code, label) in [("draft", "Draft"), ("live", "Live")] {
            let mut rec = Record::new("acme_statuses");
            rec.set("code", code).set("label", label);
            insert_missing(db, &rec, &["code"]).await?;
        }
        Ok(())
    }
}
```

`insert_missing(db, &rec, &key)` inserts the row only when no row holds the
same values in every `key` column, and returns whether it inserted. A second
run changes nothing.

## Register it

```rust
impl Module for BlogModule {
    fn seeders(&self) -> SeederSet {
        SeederSet::new("acme.blog", vec![Box::new(Statuses)])
    }
}
```

## Run seeders

```sh
lat seed
lat seed acme.blog
```

Command | Runs
--- | ---
`lat seed` | Every loaded module's seeders, in load order
`lat seed <module>` | One module's seeders
`cargo run -- seed [module]` | The same, from the application binary

Each seeder that ran prints `Seeded <module>/<name>`.
