//! Listeners the framework ships.

use async_trait::async_trait;
use chrono::Utc;

use crate::record::{ModelListener, Op, Record, SaveCx};
use crate::validation::ErrorBag;

/// Stamps `created_at` on a create and `updated_at` on every write.
///
/// Attach it per entity, never to every entity: the built-in persister writes
/// whatever the record holds, so stamping globally would send these columns to
/// tables that have none.
///
/// A `created_at` already on the record is left alone, so an import can preserve
/// the original instant.
pub struct Timestamps;

/// The column stamped once, when the row is created.
pub const CREATED_AT: &str = "created_at";
/// The column stamped on every write.
pub const UPDATED_AT: &str = "updated_at";

#[async_trait]
impl ModelListener for Timestamps {
    async fn before_save(
        &self,
        _cx: &mut SaveCx<'_>,
        rec: &mut Record,
        op: Op,
    ) -> Result<(), ErrorBag> {
        let now = Utc::now();
        if op == Op::Create && !rec.contains(CREATED_AT) {
            rec.set(CREATED_AT, now);
        }
        rec.set(UPDATED_AT, now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Actor;
    use crate::testing::connect_test;

    /// Runs `before_save` on a throwaway transaction, as the pipeline does.
    async fn stamp(rec: &mut Record, op: Op) {
        let (db, _guard) = connect_test(&[]).await;
        let mut tx = db.pool.begin().await.unwrap();
        let mut cx = SaveCx::new(db.backend, &mut tx, Actor::system("test"));
        Timestamps.before_save(&mut cx, rec, op).await.unwrap();
    }

    #[tokio::test]
    async fn a_create_stamps_both_columns() {
        let mut rec = Record::new("widgets");
        stamp(&mut rec, Op::Create).await;
        assert!(rec.datetime(CREATED_AT).is_some());
        assert!(rec.datetime(UPDATED_AT).is_some());
    }

    #[tokio::test]
    async fn an_update_stamps_only_the_updated_column() {
        let mut rec = Record::with_id("widgets", 1);
        stamp(&mut rec, Op::Update).await;
        assert!(
            rec.get(CREATED_AT).is_none(),
            "an update never sets created"
        );
        assert!(rec.datetime(UPDATED_AT).is_some());
    }

    #[tokio::test]
    async fn a_supplied_created_at_survives_an_import() {
        let earlier = Utc::now() - chrono::Duration::days(365);
        let mut rec = Record::new("widgets");
        rec.set(CREATED_AT, earlier);
        stamp(&mut rec, Op::Create).await;
        assert_eq!(rec.datetime(CREATED_AT), Some(earlier));
    }
}
