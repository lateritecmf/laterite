//! The library: folders as rows with materialized paths, links that say who
//! uses a file, and listings by folder, search, kind and sort. Imports only
//! the public API.

use laterite_core::Db;
use laterite_media::library::{self, Listing, Sort};
use laterite_media::{
    blob_path, folder, gc, ingest, link, record, stream_of, verify, FolderError, Kind, LocalDisk,
    NewMedia, NewMediaExtra, Owner, StorageDriver,
};
use std::collections::HashMap;
use std::sync::Arc;

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

async fn stored(db: &Db, disk: &LocalDisk, name: &str, bytes: &[u8], in_library: bool) -> i64 {
    let out = ingest(disk, stream_of(bytes.to_vec()), 1 << 20)
        .await
        .unwrap();
    record::create_with(
        db,
        NewMedia {
            disk: "public".into(),
            hash: out.hash,
            size: out.size as i64,
            content_type: out.content_type,
            original_name: name.into(),
            in_library,
            created_by: None,
        },
        NewMediaExtra::default(),
    )
    .await
    .unwrap()
}

/// Garbage is a record outside the library that nothing links to, past the
/// grace; a blob goes only with its last record, and a disk not given keeps its.
#[tokio::test]
async fn gc_removes_unused_records_and_the_blobs_they_last_named() {
    let (db, _g) = db().await;
    let dir = tempfile::tempdir().unwrap();
    let disk = LocalDisk::new(dir.path());
    let kept = stored(&db, &disk, "kept.txt", b"kept in the library", true).await;
    let linked = stored(&db, &disk, "linked.txt", b"linked from a post", false).await;
    let owner = Owner {
        owner_type: "acme_posts",
        owner_id: "1",
        field: "cover",
    };
    link::set(&db, owner, &[linked]).await.unwrap();
    let orphan_a = stored(&db, &disk, "orphan-a.txt", b"shared orphan bytes", false).await;
    let orphan_b = stored(&db, &disk, "orphan-b.txt", b"shared orphan bytes", false).await;
    let hash = record::find(&db, orphan_a).await.unwrap().unwrap().hash;
    let elsewhere = record::create_with(
        &db,
        NewMedia {
            disk: "s3".into(),
            hash: format!("{:0>64}", 7),
            size: 1,
            content_type: "text/plain".into(),
            original_name: "far.txt".into(),
            in_library: false,
            created_by: None,
        },
        NewMediaExtra::default(),
    )
    .await
    .unwrap();
    let disks: HashMap<String, Arc<dyn StorageDriver>> = HashMap::from([(
        "public".to_string(),
        Arc::new(disk) as Arc<dyn StorageDriver>,
    )]);

    let young = gc::collect(&db, &disks, chrono::Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(
        young,
        gc::GcReport::default(),
        "inside the grace, nothing goes"
    );

    let report = gc::collect(&db, &disks, chrono::Duration::seconds(-5))
        .await
        .unwrap();
    assert_eq!(report.records, 3);
    assert_eq!(report.blobs, 1, "one blob for the two orphans");
    assert_eq!(report.kept, [format!("s3:{:0>64}", 7)]);
    assert!(record::find(&db, kept).await.unwrap().is_some());
    assert!(record::find(&db, linked).await.unwrap().is_some());
    assert!(record::find(&db, orphan_b).await.unwrap().is_none());
    assert!(record::find(&db, elsewhere).await.unwrap().is_none());
    assert!(!dir.path().join(blob_path(&hash)).exists());
}

/// Every blob is re-hashed; a lost, altered or short one is a named fault.
#[tokio::test]
async fn verify_names_the_records_whose_bytes_do_not_match() {
    let (db, _g) = db().await;
    let dir = tempfile::tempdir().unwrap();
    let disk = LocalDisk::new(dir.path());
    let good = stored(&db, &disk, "good.txt", b"all is well", true).await;
    let rotten = stored(&db, &disk, "rotten.txt", b"these bytes will change", true).await;
    let rotten_hash = record::find(&db, rotten).await.unwrap().unwrap().hash;
    std::fs::write(dir.path().join(blob_path(&rotten_hash)), b"changed").unwrap();
    let lost = record::create_with(
        &db,
        NewMedia {
            disk: "public".into(),
            hash: format!("{:0>64}", 9),
            size: 3,
            content_type: "text/plain".into(),
            original_name: "lost.txt".into(),
            in_library: true,
            created_by: None,
        },
        NewMediaExtra::default(),
    )
    .await
    .unwrap();
    record::create_with(
        &db,
        NewMedia {
            disk: "s3".into(),
            hash: format!("{:0>64}", 8),
            size: 3,
            content_type: "text/plain".into(),
            original_name: "far.txt".into(),
            in_library: true,
            created_by: None,
        },
        NewMediaExtra::default(),
    )
    .await
    .unwrap();
    let disks: HashMap<String, Arc<dyn StorageDriver>> = HashMap::from([(
        "public".to_string(),
        Arc::new(disk) as Arc<dyn StorageDriver>,
    )]);

    let report = verify::run(&db, &disks).await.unwrap();
    assert_eq!(report.ok, 1);
    assert_eq!(report.skipped, 1);
    let faults: Vec<(i64, &str)> = report
        .faults
        .iter()
        .map(|f| (f.id, f.reason.split(':').next().unwrap()))
        .collect();
    assert_eq!(faults, [(rotten, "hash mismatch"), (lost, "missing")]);
    assert!(record::find(&db, good).await.unwrap().is_some());
}

/// The driver's default copy streams a blob to a second path.
#[tokio::test]
async fn a_disk_copies_a_blob_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let disk = LocalDisk::new(dir.path());
    disk.put("quarantine/a/b.txt", stream_of(b"moved".to_vec()))
        .await
        .unwrap();
    disk.copy("quarantine/a/b.txt", "blobs/b").await.unwrap();
    assert_eq!(std::fs::read(dir.path().join("blobs/b")).unwrap(), b"moved");
    assert_eq!(
        disk.presigned_put("x", "text/plain", std::time::Duration::from_secs(1))
            .await
            .unwrap(),
        None
    );
}
