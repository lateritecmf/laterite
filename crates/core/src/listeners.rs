//! Listeners the framework ships.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sea_query::{Alias, Expr, Query};

use crate::crypt::{Crypt, FIELD_PURPOSE};
use crate::i18n::Text;
use crate::query::{bind_values, build, text_cast};
use crate::record::{AttrValue, ModelListener, Op, Record, SaveCx};
use crate::validation::ErrorBag;
use crate::AnyRowExt;

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

/// Records who created a row and who last changed it, from the save's actor.
///
/// Stamps `created_user_id` on a create and `updated_user_id` on every write. A
/// write by a system process (an import, a console command, a content pull) sets
/// `updated_user_id` to null, so the column never credits a user with a change
/// they did not make. A value the caller set already is left alone.
pub struct Footprints;

/// The column holding the id of the user who created the row.
pub const CREATED_USER_ID: &str = "created_user_id";
/// The column holding the id of the user who last changed the row.
pub const UPDATED_USER_ID: &str = "updated_user_id";

#[async_trait]
impl ModelListener for Footprints {
    async fn before_save(
        &self,
        cx: &mut SaveCx<'_>,
        rec: &mut Record,
        op: Op,
    ) -> Result<(), ErrorBag> {
        let user = cx.actor().user_id();
        if op == Op::Create && !rec.contains(CREATED_USER_ID) {
            rec.set(CREATED_USER_ID, user);
        }
        if !rec.contains(UPDATED_USER_ID) {
            rec.set(UPDATED_USER_ID, user);
        }
        Ok(())
    }
}

/// Fills a slug column from other attributes when it is empty, unique within the
/// table.
///
/// A generated slug takes the next free numeric suffix (`launch`, `launch-2`,
/// `launch-3`) found in one query. A slug the user typed that another row holds
/// is refused with a message on that field. Rows that are trashed still hold
/// their slug.
pub struct Sluggable {
    id_field: String,
    slugs: Vec<SlugRule>,
}

/// One slug column and the attributes it is built from.
#[derive(Debug, Clone)]
pub struct SlugRule {
    /// The column the slug is written to.
    pub column: String,
    /// The attributes joined, in order, to build it.
    pub sources: Vec<String>,
}

/// The longest slug written, suffix included.
pub const SLUG_MAX: usize = 180;

impl Sluggable {
    /// A slug listener for a table keyed by `id_field`, filling each rule's column.
    pub fn new(id_field: impl Into<String>, slugs: Vec<SlugRule>) -> Self {
        Self {
            id_field: id_field.into(),
            slugs,
        }
    }

    /// Every value of `column` equal to `base` or starting with `base-`, other
    /// than the row being saved.
    async fn taken(
        &self,
        cx: &mut SaveCx<'_>,
        table: &str,
        column: &str,
        base: &str,
        own_id: Option<i64>,
    ) -> Result<Vec<String>, sqlx::Error> {
        let backend = cx.backend();
        let (sql, values) = {
            let mut select = Query::select();
            select
                .expr_as(
                    Expr::col(Alias::new(column)).cast_as(Alias::new(text_cast(backend))),
                    Alias::new("lat_slug"),
                )
                .from(Alias::new(table))
                .cond_where(
                    sea_query::Cond::any()
                        .add(Expr::col(Alias::new(column)).eq(base))
                        // A slug holds only letters, digits and hyphens, so no wildcard
                        // in `base` needs escaping.
                        .add(Expr::col(Alias::new(column)).like(format!("{base}-%"))),
                );
            if let Some(id) = own_id {
                select.and_where(Expr::col(Alias::new(&self.id_field)).ne(id));
            }
            build(backend, select)
        };
        let rows = bind_values(sqlx::query(&sql), values)
            .fetch_all(cx.conn())
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.get_text("lat_slug").ok())
            .collect())
    }
}

/// `base` cut to fit `max` characters once `suffix` is appended, never ending on
/// a separator.
fn fit(base: &str, suffix: &str, max: usize) -> String {
    let room = max.saturating_sub(suffix.len());
    let cut: String = base.chars().take(room).collect();
    format!("{}{suffix}", cut.trim_end_matches('-'))
}

/// The first of `base`, `base-2`, `base-3` and so on that `taken` does not hold.
fn next_free(base: &str, taken: &[String]) -> String {
    let first = fit(base, "", SLUG_MAX);
    if !taken.iter().any(|t| t == &first) {
        return first;
    }
    (2..)
        .map(|n| fit(base, &format!("-{n}"), SLUG_MAX))
        .find(|candidate| !taken.iter().any(|t| t == candidate))
        .unwrap_or(first)
}

#[async_trait]
impl ModelListener for Sluggable {
    async fn before_save(
        &self,
        cx: &mut SaveCx<'_>,
        rec: &mut Record,
        _op: Op,
    ) -> Result<(), ErrorBag> {
        let table = rec.entity().to_string();
        let own_id = rec.id();
        let mut bag = ErrorBag::default();
        for rule in &self.slugs {
            let typed = rec
                .text(&rule.column)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            let generated = typed.is_none();
            let base = match &typed {
                Some(value) => slug::slugify(value),
                None => {
                    let joined = rule
                        .sources
                        .iter()
                        .filter_map(|s| rec.get(s).and_then(AttrValue::to_text))
                        .collect::<Vec<_>>()
                        .join(" ");
                    slug::slugify(joined)
                }
            };
            if base.is_empty() {
                // Nothing to build from: leave an empty value as it came.
                continue;
            }
            // On an update that keeps the stored slug, there is nothing to check.
            if !generated && rec.has_original() && !rec.changed(&rule.column) {
                continue;
            }
            let taken = match self.taken(cx, &table, &rule.column, &base, own_id).await {
                Ok(taken) => taken,
                Err(e) => {
                    tracing::warn!(error = %e, table, column = rule.column, "slug lookup failed");
                    bag.add(&rule.column, Text::new("The slug could not be checked."));
                    continue;
                }
            };
            if generated {
                rec.set(rule.column.clone(), next_free(&base, &taken));
            } else if taken.iter().any(|t| t == &base) {
                bag.add(
                    &rule.column,
                    Text::new("{value} is already used by another record.")
                        .arg("value", Text::dynamic(&base)),
                );
            } else {
                rec.set(rule.column.clone(), fit(&base, "", SLUG_MAX));
            }
        }
        if bag.is_empty() {
            Ok(())
        } else {
            Err(bag)
        }
    }
}

/// Seals the named columns under the application key on every write that goes
/// through the save pipeline.
///
/// A value already sealed passes through, so a `sensitive` form field and this
/// listener compose; a null or empty value is stored as it is. The seal uses the
/// same purpose and associated data as the `sensitive` field, so each reads the
/// other's values; [`Record::opened`] reads one in code.
pub struct Encrypted {
    crypt: Arc<Crypt>,
    columns: Vec<String>,
}

impl Encrypted {
    /// A listener sealing `columns` with `crypt`.
    pub fn new(crypt: Arc<Crypt>, columns: Vec<String>) -> Self {
        Self { crypt, columns }
    }
}

#[async_trait]
impl ModelListener for Encrypted {
    async fn before_save(
        &self,
        _cx: &mut SaveCx<'_>,
        rec: &mut Record,
        _op: Op,
    ) -> Result<(), ErrorBag> {
        let mut bag = ErrorBag::default();
        for column in &self.columns {
            let Some(plain) = rec.get(column).and_then(AttrValue::to_text) else {
                continue;
            };
            if plain.is_empty()
                || self
                    .crypt
                    .decrypt(FIELD_PURPOSE, column.as_bytes(), &plain)
                    .is_ok()
            {
                continue;
            }
            match self
                .crypt
                .encrypt(FIELD_PURPOSE, column.as_bytes(), plain.as_bytes())
            {
                Ok(sealed) => {
                    rec.set(column.clone(), sealed);
                }
                Err(e) => {
                    tracing::warn!(error = %e, column, "sealing a column failed");
                    bag.add(column, Text::new("The value could not be stored securely."));
                }
            }
        }
        if bag.is_empty() {
            Ok(())
        } else {
            Err(bag)
        }
    }
}

/// Sends a record's has-many rows to the trash with it and brings them back
/// with it.
///
/// A trash stamps each live child with the parent's own `deleted_at` value; a
/// restore clears it only on children carrying that same value, so a child
/// trashed on its own before the parent stays in the trash.
pub struct SoftDeleteCascade {
    children: Vec<(String, String)>,
}

impl SoftDeleteCascade {
    /// A cascade over `children`, each a related table and its foreign key to
    /// this record.
    pub fn new(children: Vec<(String, String)>) -> Self {
        Self { children }
    }

    /// The parent's stored `deleted_at`, read in the save's transaction before
    /// the write.
    async fn stored(
        cx: &mut SaveCx<'_>,
        table: &str,
        id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let backend = cx.backend();
        let (sql, values) = {
            let mut select = Query::select();
            select
                .expr_as(
                    Expr::col(Alias::new(crate::scope::DELETED_AT))
                        .cast_as(Alias::new(text_cast(backend))),
                    Alias::new("lat_deleted"),
                )
                .from(Alias::new(table))
                .and_where(Expr::col(Alias::new("id")).eq(id));
            build(backend, select)
        };
        let row = bind_values(sqlx::query(&sql), values)
            .fetch_optional(cx.conn())
            .await?;
        Ok(row.and_then(|r| r.get_text_opt("lat_deleted").ok().flatten()))
    }
}

#[async_trait]
impl ModelListener for SoftDeleteCascade {
    async fn before_save(
        &self,
        cx: &mut SaveCx<'_>,
        rec: &mut Record,
        op: Op,
    ) -> Result<(), ErrorBag> {
        let (Op::Update, Some(id)) = (op, rec.id()) else {
            return Ok(());
        };
        let Some(value) = rec.get(crate::scope::DELETED_AT) else {
            return Ok(());
        };
        let fail = |e: sqlx::Error| {
            tracing::error!(error = %e, "cascading the trash failed");
            let mut bag = ErrorBag::default();
            bag.add(
                crate::scope::DELETED_AT,
                Text::new("The related records could not be moved with this one."),
            );
            bag
        };
        // Trash: the stamp the parent is about to carry. Restore: the stamp it
        // carries now, so only what went with it comes back.
        // `set` is the stamp to write (none clears it); `only` limits a restore
        // to the rows carrying the parent's stamp.
        let (set, only): (Option<String>, Option<String>) = match value.to_text() {
            Some(stamp) if !stamp.is_empty() => (Some(stamp), None),
            _ => {
                let table = rec.entity().to_string();
                match Self::stored(cx, &table, id).await.map_err(fail)? {
                    Some(stamp) => (None, Some(stamp)),
                    None => return Ok(()),
                }
            }
        };
        for (table, foreign_key) in &self.children {
            let backend = cx.backend();
            let (sql, values) = {
                let mut update = Query::update();
                update
                    .table(Alias::new(table))
                    .value(
                        Alias::new(crate::scope::DELETED_AT),
                        match &set {
                            Some(stamp) => stamp.as_str().into(),
                            None => sea_query::SimpleExpr::Keyword(sea_query::Keyword::Null),
                        },
                    )
                    .and_where(Expr::col(Alias::new(foreign_key)).eq(id));
                match &only {
                    Some(stamp) => update.and_where(
                        Expr::col(Alias::new(crate::scope::DELETED_AT)).eq(stamp.as_str()),
                    ),
                    None => {
                        update.and_where(Expr::col(Alias::new(crate::scope::DELETED_AT)).is_null())
                    }
                };
                build(backend, update)
            };
            bind_values(sqlx::query(&sql), values)
                .execute(cx.conn())
                .await
                .map_err(fail)?;
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "testing"))]
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

    #[tokio::test]
    async fn a_system_write_clears_the_updated_user() {
        let (db, _guard) = connect_test(&[]).await;
        let mut tx = db.pool.begin().await.unwrap();
        let mut rec = Record::with_id("widgets", 1);
        let mut cx = SaveCx::new(db.backend, &mut tx, Actor::system("import"));
        Footprints
            .before_save(&mut cx, &mut rec, Op::Update)
            .await
            .unwrap();
        assert!(rec.get(UPDATED_USER_ID).is_some_and(AttrValue::is_null));
        assert!(
            rec.get(CREATED_USER_ID).is_none(),
            "an update never sets created"
        );

        let mut rec = Record::new("widgets");
        let mut cx = SaveCx::new(db.backend, &mut tx, Actor::user(7, "ada"));
        Footprints
            .before_save(&mut cx, &mut rec, Op::Create)
            .await
            .unwrap();
        assert_eq!(rec.int(CREATED_USER_ID), Some(7));
        assert_eq!(rec.int(UPDATED_USER_ID), Some(7));
    }

    #[test]
    fn the_next_free_suffix_skips_every_taken_one() {
        let taken = ["launch", "launch-2", "launch-4"].map(String::from);
        assert_eq!(next_free("launch", &taken), "launch-3");
        assert_eq!(next_free("notes", &taken), "notes");
    }

    #[test]
    fn the_cap_covers_the_suffix() {
        let long = "a".repeat(SLUG_MAX + 20);
        let taken = [fit(&long, "", SLUG_MAX)];
        let slug = next_free(&long, &taken);
        assert_eq!(slug.len(), SLUG_MAX);
        assert!(slug.ends_with("-2"));
    }
}
