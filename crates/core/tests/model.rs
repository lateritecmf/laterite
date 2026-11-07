//! Typed models through `#[derive(Model)]`: read, write and batched loads.
use chrono::{DateTime, Utc};
use laterite_core::model::{self, Model};
use laterite_core::strata::*;
use laterite_core::testing::connect_test;
use laterite_core::MigrationSet;
use sea_query::Alias;

#[derive(Debug, Clone, PartialEq, laterite_core::Model)]
#[model(table = "posts")]
struct Post {
    id: i64,
    title: String,
    views: i64,
    published: bool,
    #[model(column = "published_at")]
    at: Option<DateTime<Utc>>,
    rating: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, laterite_core::Model)]
#[model(table = "comments")]
struct Comment {
    id: i64,
    post_id: i64,
    body: String,
}

struct Create;

#[async_trait(?Send)]
impl Migration for Create {
    fn name(&self) -> &str {
        "0001_create"
    }

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        let id = || {
            ColumnDef::new(Alias::new("id"))
                .big_integer()
                .not_null()
                .auto_increment()
                .primary_key()
                .to_owned()
        };
        s.exec(
            Table::create()
                .table(Alias::new("posts"))
                .col(id())
                .col(ColumnDef::new(Alias::new("title")).text().not_null())
                .col(ColumnDef::new(Alias::new("views")).big_integer().not_null())
                .col(bool_col(Alias::new("published")).not_null())
                .col(ColumnDef::new(Alias::new("published_at")).text().null())
                .col(ColumnDef::new(Alias::new("rating")).double().null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("comments"))
                .col(id())
                .col(
                    ColumnDef::new(Alias::new("post_id"))
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(Alias::new("body")).text().not_null())
                .to_owned(),
        )
        .await
    }
}

fn post(title: &str) -> Post {
    Post {
        id: 0,
        title: title.to_string(),
        views: 3,
        published: true,
        at: Some("2026-11-06T10:00:00Z".parse().unwrap()),
        rating: Some(4.5),
    }
}

#[tokio::test]
async fn a_model_round_trips_through_insert_find_and_update() {
    let (db, _guard) =
        connect_test(&[MigrationSet::new("acme.blog", vec![Box::new(Create)])]).await;
    assert_eq!(Post::COLUMNS[0], "id");

    let id = model::insert(&db, &post("Hello")).await.unwrap();
    let mut found = model::find::<Post>(&db, id).await.unwrap().unwrap();
    assert_eq!(
        found,
        Post {
            id,
            ..post("Hello")
        }
    );

    found.title = "Hello again".into();
    found.published = false;
    found.at = None;
    found.rating = None;
    model::update(&db, &found).await.unwrap();
    assert_eq!(model::find::<Post>(&db, id).await.unwrap().unwrap(), found);
    assert!(model::find::<Post>(&db, id + 99).await.unwrap().is_none());
}

#[tokio::test]
async fn relations_load_for_a_whole_page_in_one_query_each() {
    let (db, _guard) =
        connect_test(&[MigrationSet::new("acme.blog", vec![Box::new(Create)])]).await;
    let a = model::insert(&db, &post("A")).await.unwrap();
    let b = model::insert(&db, &post("B")).await.unwrap();
    for (post_id, body) in [(a, "one"), (a, "two"), (b, "three")] {
        model::insert(
            &db,
            &Comment {
                id: 0,
                post_id,
                body: body.into(),
            },
        )
        .await
        .unwrap();
    }
    let posts = model::all::<Post>(&db).await.unwrap();
    let ids: Vec<i64> = posts.iter().map(Model::id).collect();
    let comments = model::has_many::<Comment>(&db, "post_id", &ids, |c| c.post_id)
        .await
        .unwrap();
    assert_eq!(
        comments[&a]
            .iter()
            .map(|c| c.body.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert_eq!(comments[&b].len(), 1);

    let by_id = model::find_many::<Post>(&db, &[b]).await.unwrap();
    assert_eq!(by_id[&b].title, "B");
}
