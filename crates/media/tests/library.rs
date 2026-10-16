//! The library: folders as rows with materialized paths, links that say who
//! uses a file, and listings by folder, search, kind and sort. Imports only
//! the public API.

use laterite_core::Db;
use laterite_media::library::{self, Listing, Sort};
use laterite_media::{folder, link, record, FolderError, Kind, NewMedia, NewMediaExtra, Owner};

async fn db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[laterite_media::migrations::migrations()]).await
}

async fn file(db: &Db, name: &str, ct: &str, folder: Option<i64>, in_library: bool) -> i64 {
    record::create_with(
        db,
        NewMedia {
            disk: "public".into(),
            hash: format!("{:0>64}", name.len()),
            size: name.len() as i64,
            content_type: ct.into(),
            original_name: name.into(),
            in_library,
            created_by: None,
        },
        NewMediaExtra {
            folder_id: folder,
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn folders_nest_rename_move_and_refuse_a_cycle() {
    let (db, _g) = db().await;
    let photos = folder::create(&db, None, "Photos").await.unwrap();
    assert_eq!(photos.path, "/Photos");
    let y = folder::create(&db, Some(photos.id), "2026").await.unwrap();
    assert_eq!(y.path, "/Photos/2026");
    let docs = folder::create(&db, None, "Docs").await.unwrap();
    assert!(matches!(
        folder::create(&db, None, "photos").await,
        Err(FolderError::Taken(_))
    ));
    assert!(matches!(
        folder::create(&db, None, "a/b").await,
        Err(FolderError::Name(_))
    ));

    let renamed = folder::rename(&db, photos.id, "Pictures").await.unwrap();
    assert_eq!(renamed.path, "/Pictures");
    assert_eq!(
        folder::find(&db, y.id).await.unwrap().unwrap().path,
        "/Pictures/2026"
    );

    let moved = folder::move_to(&db, y.id, Some(docs.id)).await.unwrap();
    assert_eq!(moved.path, "/Docs/2026");
    assert!(matches!(
        folder::move_to(&db, docs.id, Some(y.id)).await,
        Err(FolderError::Cycle)
    ));
    let chain = folder::ancestors(&db, y.id).await.unwrap();
    assert_eq!(
        chain.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["Docs", "2026"]
    );

    file(&db, "a.png", "image/png", Some(y.id), true).await;
    assert!(matches!(
        folder::delete(&db, y.id).await,
        Err(FolderError::NotEmpty {
            files: 1,
            folders: 0
        })
    ));
    assert!(matches!(
        folder::delete(&db, docs.id).await,
        Err(FolderError::NotEmpty {
            files: 0,
            folders: 1
        })
    ));
    folder::delete(&db, renamed.id).await.unwrap();
    assert_eq!(folder::children(&db, None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn links_say_who_uses_a_file_and_replace_in_order() {
    let (db, _g) = db().await;
    let a = file(&db, "a.png", "image/png", None, false).await;
    let b = file(&db, "bb.png", "image/png", None, false).await;
    let owner = Owner {
        owner_type: "acme_posts",
        owner_id: "7",
        field: "gallery",
    };
    link::set(&db, owner, &[b, a]).await.unwrap();
    let links = link::of_owner(&db, owner).await.unwrap();
    assert_eq!(links.iter().map(|l| l.media_id).collect::<Vec<_>>(), [b, a]);
    link::set(&db, owner, &[a]).await.unwrap();
    assert_eq!(link::of_owner(&db, owner).await.unwrap().len(), 1);
    assert_eq!(link::users_of(&db, a).await.unwrap()[0].owner_id, "7");
    assert!(link::users_of(&db, b).await.unwrap().is_empty());
    assert_eq!(link::clear_owner(&db, "acme_posts", "7").await.unwrap(), 1);
    assert!(link::of_owner(&db, owner).await.unwrap().is_empty());
    link::set(&db, owner, &[a]).await.unwrap();
    record::delete(&db, a).await.unwrap();
    assert!(link::users_of(&db, a).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_library_lists_by_folder_search_kind_and_sort() {
    let (db, _g) = db().await;
    let photos = folder::create(&db, None, "Photos").await.unwrap();
    let cat = file(&db, "cat.png", "image/png", None, true).await;
    let _dog = file(&db, "dog-photo.jpg", "image/jpeg", Some(photos.id), true).await;
    let _notes = file(&db, "notes.pdf", "application/pdf", None, true).await;
    let hidden = file(&db, "upload.png", "image/png", None, false).await;
    record::update_details(&db, cat, Some("Our cat"), Some("A tabby"))
        .await
        .unwrap();

    let root = library::list(
        &db,
        &Listing {
            per_page: 50,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(root.total, 2, "the root, library files only");
    assert_eq!(root.items[0].original_name, "notes.pdf", "newest first");

    let in_photos = library::list(
        &db,
        &Listing {
            folder: Some(photos.id),
            per_page: 50,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(in_photos.total, 1);
    assert_eq!(in_photos.items[0].kind(), Kind::Image);

    let found = library::list(
        &db,
        &Listing {
            search: Some("TABBY".into()),
            per_page: 50,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(found.total, 1);
    assert_eq!(found.items[0].display_name(), "Our cat");
    let across = library::list(
        &db,
        &Listing {
            search: Some("photo".into()),
            per_page: 50,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(across.total, 1, "search looks across folders");

    let docs = library::list(
        &db,
        &Listing {
            kind: Some(Kind::Document),
            per_page: 50,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(docs.total, 1);
    let by_name = library::list(
        &db,
        &Listing {
            sort: Sort::Name,
            per_page: 1,
            page: 2,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(by_name.total, 2);
    assert_eq!(by_name.items[0].original_name, "notes.pdf");

    record::move_to_folder(&db, cat, Some(photos.id))
        .await
        .unwrap();
    record::set_in_library(&db, hidden, true, None)
        .await
        .unwrap();
    let root = library::list(
        &db,
        &Listing {
            per_page: 50,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        root.items
            .iter()
            .map(|r| r.original_name.as_str())
            .collect::<Vec<_>>(),
        ["upload.png", "notes.pdf"]
    );
    let one = record::find(&db, cat).await.unwrap().unwrap();
    assert_eq!(one.folder_id, Some(photos.id));
    assert_eq!(one.meta, serde_json::json!({}));
}
