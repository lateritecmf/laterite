//! The media library: uploads into folders, the browser, the serve route and
//! deletion against links. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{
    password, store as auth_store, AuthConfig, AuthService, NewOperator, RequestContext,
};
use laterite_core::{CatalogStore, Db};
use laterite_media::{folder, link, record, DiskReg, LocalDisk, Owner};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const BOUNDARY: &str = "lat-boundary";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: &Db, root: &std::path::Path) -> Router {
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    let mut registry = laterite_core::Registry::new();
    registry.set_owner(laterite_core::ModuleId::new("acme.app"));
    registry.add(DiskReg::new(
        "public",
        Arc::new(LocalDisk::new(root.join("media"))),
    ));
    let contributions = Contributions {
        plugin_defined: Arc::new(registry),
        ..Default::default()
    };
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        contributions,
        config,
        Arc::new(CatalogStore::default()),
    )
}

async fn sessions(db: &Db) -> (String, String) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    svc.create_superuser(NewOperator {
        username: "root",
        email: "root@acme.test",
        first_name: "Root",
        last_name: None,
        password: "rootpw12345",
        timezone: None,
    })
    .await
    .unwrap();
    let hash = password::hash_password("plainpw12345").unwrap();
    auth_store::create_user(db, "plain", "plain@acme.test", "Plain", None, &hash, false)
        .await
        .unwrap();
    let mut tokens = Vec::new();
    for (user, pw) in [("root", "rootpw12345"), ("plain", "plainpw12345")] {
        tokens.push(
            svc.authenticate(user, pw, &RequestContext::default())
                .await
                .unwrap()
                .token,
        );
    }
    svc.set_session_data(&tokens[0], r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    (tokens.remove(0), tokens.remove(0))
}

async fn get(
    app: &Router,
    path: &str,
    session: Option<&str>,
) -> (StatusCode, String, Vec<(String, String)>) {
    let mut req = Request::builder().uri(path);
    if let Some(s) = session {
        req = req.header("cookie", format!("{SESSION_COOKIE}={s}"));
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string(), headers)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

async fn post(app: &Router, path: &str, session: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, location)
}

/// A PNG header plus padding: sniffs as an image.
fn png(extra: u8) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend(std::iter::repeat_n(extra, 64));
    bytes
}

fn multipart(fields: &[(&str, &str)], files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in fields {
        out.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
        );
    }
    for (filename, bytes) in files {
        out.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n")
                .as_bytes(),
        );
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    out
}

async fn upload(
    app: &Router,
    session: &str,
    fields: &[(&str, &str)],
    files: &[(&str, &[u8])],
) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri("/admin/media/upload")
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(multipart(fields, files)))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, location)
}

#[tokio::test]
async fn files_upload_into_folders_and_serve_from_their_hash() {
    let (db, _g) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let (root, plain) = sessions(&db).await;

    let (status, html, _) = get(&app, "/admin/media", Some(&root)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("No files here yet.") && html.contains("New folder"),
        "{html}"
    );
    let (status, _, _) = get(&app, "/admin/media", Some(&plain)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, location) = post(&app, "/admin/media/folders", &root, "name=Photos").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let photos = folder::children(&db, None).await.unwrap()[0].clone();
    assert_eq!(location, format!("/admin/media?folder={}", photos.id));

    let (status, location) = upload(
        &app,
        &root,
        &[("folder", &photos.id.to_string())],
        &[
            ("cat photo.png", &png(1)),
            ("same.png", &png(1)),
            ("notes.txt", b"hello world, a text file"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, format!("/admin/media?folder={}", photos.id));
    let listed = laterite_media::library::list(
        &db,
        &laterite_media::library::Listing {
            folder: Some(photos.id),
            per_page: 50,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(listed.total, 3);
    let cat = listed
        .items
        .iter()
        .find(|r| r.original_name == "cat photo.png")
        .unwrap();
    let same = listed
        .items
        .iter()
        .find(|r| r.original_name == "same.png")
        .unwrap();
    assert_eq!(cat.hash, same.hash, "identical bytes share a blob");
    assert_eq!(cat.content_type, "image/png");
    assert!(cat.in_library);

    let (_, html, _) = get(
        &app,
        &format!("/admin/media?folder={}", photos.id),
        Some(&root),
    )
    .await;
    assert!(
        html.contains("cat photo.png") && html.contains("3 file(s)"),
        "{html}"
    );
    let (_, html, _) = get(&app, "/admin/media?q=notes", Some(&root)).await;
    assert!(
        html.contains("notes.txt") && !html.contains("cat photo"),
        "{html}"
    );

    // The serve route: by hash, immutable, with a validator.
    let url = format!("/storage/media/{}/cat-photo.png", cat.hash);
    let (status, body, headers) = get(&app, &url, None).await;
    assert_eq!(status, StatusCode::OK);
    // The body is read as text, where the PNG's first byte widens; the
    // declared length is the blob's.
    assert_eq!(header(&headers, "content-length"), Some("72"));
    assert!(body.ends_with("\u{1}\u{1}"));
    assert_eq!(header(&headers, "content-type"), Some("image/png"));
    assert_eq!(
        header(&headers, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(header(&headers, "x-content-type-options"), Some("nosniff"));
    let etag = header(&headers, "etag").unwrap().to_string();
    let req = Request::builder()
        .uri(&url)
        .header("if-none-match", &etag)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::NOT_MODIFIED
    );
    let (status, _, _) = get(&app, "/storage/media/0000/x.png", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The file page edits details and moves between folders.
    let (status, html, _) = get(&app, &format!("/admin/media/{}", cat.id), Some(&root)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("No record links to this file."), "{html}");
    post(
        &app,
        &format!("/admin/media/{}/update", cat.id),
        &root,
        "name=cat.png&title=Our+cat&alt=A+tabby",
    )
    .await;
    let updated = record::find(&db, cat.id).await.unwrap().unwrap();
    assert_eq!(updated.original_name, "cat.png");
    assert_eq!(updated.title.as_deref(), Some("Our cat"));
    post(&app, &format!("/admin/media/{}/move", cat.id), &root, "to=").await;
    assert_eq!(
        record::find(&db, cat.id).await.unwrap().unwrap().folder_id,
        None
    );

    // Deleting a linked file shows who uses it; a superuser may still delete.
    link::set(
        &db,
        Owner {
            owner_type: "acme_posts",
            owner_id: "9",
            field: "cover",
        },
        &[same.id],
    )
    .await
    .unwrap();
    let (_, html, _) = get(&app, &format!("/admin/media/{}", same.id), Some(&root)).await;
    assert!(
        html.contains("acme_posts") && html.contains("Delete it anyway?"),
        "{html}"
    );
    let (status, _) = post(&app, &format!("/admin/media/{}/delete", same.id), &root, "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(record::find(&db, same.id).await.unwrap().is_none());
    // The blob stays while another record shares it.
    let (status, _, _) = get(&app, &url, None).await;
    assert_eq!(status, StatusCode::OK);
    post(&app, &format!("/admin/media/{}/delete", cat.id), &root, "").await;
    let (status, _, _) = get(&app, &url, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the last record takes the blob with it"
    );

    // A folder with files cannot go; an empty one can.
    let (_, location) = post(
        &app,
        &format!("/admin/media/folders/{}/delete", photos.id),
        &root,
        "",
    )
    .await;
    assert_eq!(
        location,
        format!("/admin/media?folder={}", photos.id),
        "refused: still holds notes.txt"
    );
    let (_, html, _) = get(
        &app,
        &format!("/admin/media?folder={}", photos.id),
        Some(&root),
    )
    .await;
    assert!(html.contains("still holds 1 file(s)"), "{html}");
}
