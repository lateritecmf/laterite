//! A file on the private disk opens inside the admin alone, for a signed-in
//! operator who may see a record it is attached to or browse the library it
//! sits in. Imports only the public API.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::medialib::PRIVATE_DISK;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{password, store, AuthConfig, AuthService, RequestContext};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet};
use laterite_media::{ingest, link, record, stream_of, LocalDisk, NewMedia, Owner};
use sea_query::Alias;
use tower::ServiceExt;

const SECRET: &[u8] = b"proof: seen at the door";

struct CreatePosts;

#[async_trait(?Send)]
impl Migration for CreatePosts {
    fn name(&self) -> &str {
        "0001_create_posts"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("posts"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("code")).text().not_null())
                .col(ColumnDef::new(Alias::new("cover_id")).big_integer())
                .col(ColumnDef::new(Alias::new("gallery")).text())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.posts", vec![Box::new(CreatePosts)]));
    laterite_core::testing::connect_test(&sets).await
}

fn app(db: &Db, storage: &std::path::Path) -> Router {
    let mut config = AdminConfig::default();
    config.storage_dir = storage.to_string_lossy().to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/media_field.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

/// A file on the private disk holding [`SECRET`] under `salt`, in the library
/// or out of it.
async fn private_file(db: &Db, storage: &std::path::Path, salt: &str, in_library: bool) -> i64 {
    let disk = LocalDisk::new(storage.join(PRIVATE_DISK).to_string_lossy().to_string());
    let bytes = [SECRET, salt.as_bytes()].concat();
    let stored = ingest(&disk, stream_of(bytes), 1024 * 1024).await.unwrap();
    record::create(
        db,
        NewMedia {
            disk: PRIVATE_DISK.to_string(),
            hash: stored.hash,
            size: stored.size as i64,
            content_type: "text/plain".into(),
            original_name: format!("{salt}.txt"),
            in_library,
            created_by: None,
        },
    )
    .await
    .unwrap()
}

/// An account holding exactly `grants` (a superuser when `None`), signed in.
async fn operator(db: &Db, name: &str, grants: Option<&[&str]>) -> String {
    let hash = password::hash_password("operatorpw12345").unwrap();
    let id = store::create_user(
        db,
        name,
        &format!("{name}@acme.test"),
        name,
        None,
        &hash,
        grants.is_none(),
    )
    .await
    .unwrap();
    if let Some(grants) = grants.filter(|g| !g.is_empty()) {
        let grants: Vec<String> = grants.iter().map(|g| String::from(*g)).collect();
        let role = store::create_role(db, &format!("{name}_role"), "Their role", &grants)
            .await
            .unwrap();
        store::assign_role(db, id, role).await.unwrap();
    }
    AuthService::new(db.clone(), AuthConfig::default())
        .authenticate(name, "operatorpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token
}

async fn get(app: &Router, path: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut request = Request::builder().uri(path);
    if let Some(token) = token {
        request = request.header("cookie", format!("laterite_session={token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn a_private_file_opens_only_for_who_may_see_its_record() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let attached = private_file(&db, dir.path(), "attached", false).await;
    let loose = private_file(&db, dir.path(), "loose", false).await;
    let shelved = private_file(&db, dir.path(), "shelved", true).await;
    link::set(
        &db,
        Owner {
            owner_type: "posts",
            owner_id: "1",
            field: "gallery",
        },
        &[attached],
    )
    .await
    .unwrap();

    let root = operator(&db, "root", None).await;
    let editor = operator(&db, "editor", Some(&["acme.manage_posts"])).await;
    let librarian = operator(&db, "librarian", Some(&["media.browse"])).await;
    let nobody = operator(&db, "nobody", Some(&[])).await;
    let at = |id: i64, name: &str| format!("/admin/media-files/{id}/{name}.txt");
    let opens = |status: StatusCode, body: &str| status == StatusCode::OK && body.contains("proof");

    // Signed out: the sign-in page, never the file. The public route holds
    // nothing of it without a signature either.
    let (status, body) = get(&app, &at(attached, "attached"), None).await;
    assert!(
        status.is_redirection() && !body.contains("proof"),
        "{status}"
    );
    let hash = record::find(&db, attached).await.unwrap().unwrap().hash;
    let (status, body) = get(&app, &format!("/storage/media/{hash}/attached.txt"), None).await;
    assert!(
        status == StatusCode::NOT_FOUND && !body.contains("proof"),
        "{status}"
    );

    // A file attached to a record: whoever may see the record, and nobody else.
    for (who, token, expected) in [
        ("root", &root, true),
        ("editor", &editor, true),
        ("librarian", &librarian, false),
        ("nobody", &nobody, false),
    ] {
        let (status, body) = get(&app, &at(attached, "attached"), Some(token)).await;
        assert_eq!(opens(status, &body), expected, "{who}: {status}");
        if !expected {
            assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
        }
    }
    // A file attached to nothing, outside the library: a superuser alone.
    for (who, token, expected) in [("root", &root, true), ("editor", &editor, false)] {
        let (status, body) = get(&app, &at(loose, "loose"), Some(token)).await;
        assert_eq!(opens(status, &body), expected, "{who}: {status}");
    }
    // A private library file: whoever may browse the library.
    for (who, token, expected) in [("librarian", &librarian, true), ("editor", &editor, false)] {
        let (status, body) = get(&app, &at(shelved, "shelved"), Some(token)).await;
        assert_eq!(opens(status, &body), expected, "{who}: {status}");
    }
    let (_, headers_probe) = get(&app, &at(attached, "attached"), Some(&editor)).await;
    assert!(headers_probe.contains("seen at the door"));

    // The field's tiles point inside the admin, carry no signature, and leave
    // out a file the asker may not open.
    let lookup = format!("/admin/media-files/resolve?ids={attached},{loose},{shelved}");
    let (status, json) = get(&app, &lookup, Some(&editor)).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(json.contains(&at(attached, "attached")), "{json}");
    assert!(
        !json.contains("loose") && !json.contains("shelved"),
        "{json}"
    );
    assert!(
        !json.contains("/storage/media") && !json.contains("?e="),
        "{json}"
    );
    let (status, _) = get(&app, &lookup, None).await;
    assert!(status.is_redirection());
}
