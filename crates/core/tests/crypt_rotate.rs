//! A key rotation re-seals every registered column under the current key and
//! leaves what it cannot open. Imports only the public API.

use laterite_core::crypt::{self, CryptModule};
use laterite_core::strata::*;
use laterite_core::{Crypt, MasterKey, Module};
use sea_query::Alias;

/// A table with one sealed column, as an application would declare it.
struct Secrets;

#[async_trait(?Send)]
impl laterite_core::Migration for Secrets {
    fn name(&self) -> &str {
        "0001_create_secrets"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("secrets"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("token")).text())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (laterite_core::Db, laterite_core::testing::TestGuard) {
    let app = MigrationSet::new("acme", vec![Box::new(Secrets)]);
    laterite_core::testing::connect_test(&[CryptModule.migrations(), app]).await
}

async fn insert(db: &laterite_core::Db, token: Option<&str>) -> i64 {
    let stmt = Query::insert()
        .into_table(Alias::new("secrets"))
        .columns([Alias::new("token")])
        .values_panic([token.into()])
        .to_owned();
    insert_returning_id(db, stmt, Alias::new("id"))
        .await
        .unwrap()
}

async fn token_of(db: &laterite_core::Db, id: i64) -> Option<String> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Alias::new("token"))
            .from(Alias::new("secrets"))
            .and_where(Expr::col(Alias::new("id")).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    row.get_text_opt("token").unwrap()
}

#[tokio::test]
async fn a_rotation_reseals_the_registered_columns_and_counts_what_it_cannot_open() {
    let (db, _guard) = test_db().await;
    crypt::register_column(&db, "secrets", "token", crypt::FIELD_PURPOSE)
        .await
        .unwrap();
    crypt::register_column(&db, "secrets", "token", crypt::FIELD_PURPOSE)
        .await
        .unwrap();
    assert_eq!(crypt::registered_columns(&db).await.unwrap().len(), 1);

    let old = MasterKey::generate(1);
    let old_export = old.export();
    let before = Crypt::new(old, Vec::new());
    let sealed_old = before
        .encrypt(crypt::FIELD_PURPOSE, b"token", b"hunter2")
        .unwrap();
    let stranger = Crypt::ephemeral()
        .encrypt(crypt::FIELD_PURPOSE, b"token", b"nobody")
        .unwrap();
    let a = insert(&db, Some(&sealed_old)).await;
    let b = insert(&db, Some(&stranger)).await;
    let c = insert(&db, None).await;
    let d = insert(&db, Some("")).await;

    let after = Crypt::new(
        MasterKey::generate(2),
        vec![MasterKey::parse(&old_export).unwrap()],
    );
    let done = crypt::rotate(&db, &after).await.unwrap();
    assert_eq!(done.resealed, 1);
    assert_eq!(done.unreadable, 1);
    assert_eq!(done.current, 0);

    let resealed = token_of(&db, a).await.unwrap();
    assert_ne!(resealed, sealed_old);
    assert_eq!(after.key_id_of(&resealed), Some(2));
    assert_eq!(
        after
            .decrypt(crypt::FIELD_PURPOSE, b"token", &resealed)
            .unwrap(),
        b"hunter2"
    );
    assert_eq!(token_of(&db, b).await.unwrap(), stranger);
    assert_eq!(token_of(&db, c).await, None);
    assert_eq!(token_of(&db, d).await.unwrap(), "");

    // A second pass finds everything already under the current key.
    let again = crypt::rotate(&db, &after).await.unwrap();
    assert_eq!(again.resealed, 0);
    assert_eq!(again.current, 1);
    assert_eq!(again.unreadable, 1);
}
