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
use laterite_media::{
    blob_path, folder, ingest, link, record, stream_of, DiskReg, LocalDisk, NewMedia,
    NewMediaExtra, Owner, StorageDriver,
};
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
    config.storage_dir = root.display().to_string();
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

/// A file on a private disk never serves bare. The admin's own link to it is
/// signed, bound to the hash and timed; a tampered or retargeted token fails.
#[tokio::test]
async fn a_private_file_serves_only_behind_a_live_signature() {
    let (db, _g) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let (root, _) = sessions(&db).await;

    let private = LocalDisk::new(dir.path().join("private"));
    let stored = ingest(&private, stream_of(b"secret report".to_vec()), 1 << 20)
        .await
        .unwrap();
    assert!(dir
        .path()
        .join("private")
        .join(blob_path(&stored.hash))
        .is_file());
    let id = record::create_with(
        &db,
        NewMedia {
            disk: "private".into(),
            hash: stored.hash.clone(),
            size: stored.size as i64,
            content_type: stored.content_type.clone(),
            original_name: "report.txt".into(),
            in_library: true,
            created_by: None,
        },
        NewMediaExtra::default(),
    )
    .await
    .unwrap();

    let bare = format!("/storage/media/{}/report.txt", stored.hash);
    let (status, _, _) = get(&app, &bare, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no token, no file");

    let (status, html, _) = get(&app, &format!("/admin/media/{id}"), Some(&root)).await;
    assert_eq!(status, StatusCode::OK);
    let start = html.find(&format!("{bare}?e=")).expect("a signed link");
    let end = html[start..].find('"').unwrap();
    let signed = html[start..start + end]
        .replace("&amp;", "&")
        .replace("&#38;", "&");
    let (status, body, headers) = get(&app, &signed, None).await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    assert_eq!(body, "secret report");
    assert_eq!(header(&headers, "cache-control"), Some("private, no-store"));

    let (status, _, _) = get(&app, &format!("{signed}0"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a changed signature");
    let expired = signed.replacen("?e=", "?e=1", 1);
    let (status, _, _) = get(&app, &expired, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a changed expiry");
    let other = ingest(&private, stream_of(b"another".to_vec()), 1 << 20)
        .await
        .unwrap();
    let retargeted = signed.replace(&stored.hash, &other.hash);
    let (status, _, _) = get(&app, &retargeted, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the token is bound to its hash"
    );
}

/// The uploader widget sends one file per request and reads a JSON answer
/// naming what was stored and what was refused, with no redirect.
#[tokio::test]
async fn the_uploader_gets_a_json_answer_per_file() {
    let (db, _g) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let (root, _) = sessions(&db).await;
    let req = Request::builder()
        .method("POST")
        .uri("/admin/media/upload")
        .header("cookie", format!("{SESSION_COOKIE}={root}"))
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .header("x-requested-with", "fetch")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(multipart(&[], &[("dropped.png", &png(9))])))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["stored"][0]["name"], "dropped.png");
    assert!(json["stored"][0]["url"]
        .as_str()
        .unwrap()
        .starts_with("/storage/media/"));
    assert_eq!(json["refused"].as_array().unwrap().len(), 0);
    let (status, html, _) = get(&app, "/admin/media", Some(&root)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("dropped.png") && html.contains("lat-upload__zone"),
        "{html}"
    );
}

/// A variant is made on first request from a named preset, cached, served
/// with its own validator, and refused for a name nobody declared.
#[tokio::test]
async fn a_thumbnail_is_made_once_from_its_preset() {
    let (db, _g) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let (root, _) = sessions(&db).await;
    let (status, _) = upload(&app, &root, &[], &[("wide.png", &wide_png(400, 200))]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let listed = laterite_media::library::list(&db, &laterite_media::library::Listing::default())
        .await
        .unwrap();
    let r = &listed.items[0];
    assert_eq!(
        (r.width, r.height),
        (Some(400), Some(200)),
        "measured on upload"
    );

    let url = format!("/storage/media/{}/thumb/wide.png", r.hash);
    let (status, body, headers) = get(&app, &url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header(&headers, "content-type"), Some("image/jpeg"));
    assert_eq!(
        header(&headers, "etag"),
        Some(format!("\"{}/thumb\"", r.hash).as_str())
    );
    let made = laterite_media::variant::find(&db, r.id, "thumb")
        .await
        .unwrap()
        .expect("a variant row");
    assert_eq!((made.width, made.height), (Some(320), Some(160)));
    assert_eq!(
        header(&headers, "content-length").and_then(|v| v.parse::<i64>().ok()),
        Some(made.size)
    );
    let _ = body;
    let (status, _, _) = get(&app, &url, None).await;
    assert_eq!(status, StatusCode::OK, "served again from the cache");
    assert_eq!(
        laterite_media::variant::list_for(&db, r.id)
            .await
            .unwrap()
            .len(),
        1
    );
    let (status, _, _) = get(
        &app,
        &format!("/storage/media/{}/giant/wide.png", r.hash),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an undeclared preset");
    let (status, _, _) = get(
        &app,
        &format!("/storage/media/{}/thumb/x.txt", "0".repeat(64)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A real PNG of the given size, so a variant has pixels to resize.
fn wide_png(w: u32, h: u32) -> Vec<u8> {
    use std::io::Write;
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    0xEDB8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let mut raw = Vec::new();
    for _ in 0..h {
        // A filter byte, then the row's pixels.
        raw.extend(std::iter::once(0u8).chain((0..w).flat_map(|x| [(x % 256) as u8, 40, 30])));
    }
    // Stored, uncompressed deflate blocks (65535 bytes at most each).
    let mut deflated = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        deflated.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        deflated.extend_from_slice(&len.to_le_bytes());
        deflated.extend_from_slice(&(!len).to_le_bytes());
        deflated.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    deflated.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.write_all(&w.to_be_bytes()).unwrap();
    ihdr.write_all(&h.to_be_bytes()).unwrap();
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &deflated);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A crop keeps the format, lands as a new file in the folder by default, and
/// rewrites the original in place when asked, dropping its variants.
#[tokio::test]
async fn an_image_is_cropped_to_a_new_file_or_in_place() {
    let (db, _g) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let (root, _) = sessions(&db).await;
    let (status, _) = upload(&app, &root, &[], &[("wide.png", &wide_png(400, 200))]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let listed = laterite_media::library::list(&db, &laterite_media::library::Listing::default())
        .await
        .unwrap();
    let r = listed.items[0].clone();
    let (status, _, _) = get(
        &app,
        &format!("/storage/media/{}/thumb/wide.png", r.hash),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a variant exists before the replace"
    );

    let (status, location) = post(
        &app,
        &format!("/admin/media/{}/crop", r.id),
        &root,
        "x=100&y=50&w=200&h=100&mode=new",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let new_id: i64 = location.rsplit('/').next().unwrap().parse().unwrap();
    assert_ne!(new_id, r.id);
    let copy = record::find(&db, new_id).await.unwrap().unwrap();
    assert_eq!(copy.original_name, "wide-crop.png");
    assert_eq!((copy.width, copy.height), (Some(200), Some(100)));
    assert_eq!(copy.content_type, "image/png");
    assert_eq!(copy.folder_id, r.folder_id);
    assert!(dir
        .path()
        .join("media")
        .join(blob_path(&copy.hash))
        .is_file());

    let (status, location) = post(
        &app,
        &format!("/admin/media/{}/crop", r.id),
        &root,
        "x=0&y=0&w=100&h=100&mode=replace",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, format!("/admin/media/{}", r.id));
    let same = record::find(&db, r.id).await.unwrap().unwrap();
    assert_ne!(same.hash, r.hash, "new bytes, new hash");
    assert_eq!((same.width, same.height), (Some(100), Some(100)));
    assert!(
        laterite_media::variant::list_for(&db, r.id)
            .await
            .unwrap()
            .is_empty(),
        "variants of the old bytes are gone"
    );
    assert!(
        !dir.path().join("media").join(blob_path(&r.hash)).exists(),
        "the old blob had no other record"
    );
    let (status, html, _) = get(&app, &format!("/admin/media/{}", r.id), Some(&root)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Image cropped.") && html.contains(r#"id="crop-image""#),
        "{html}"
    );
}

/// A direct upload: the local disk cannot presign, so the uploader is told to
/// post instead; a quarantined object handed back is hashed, stored under its
/// hash, recorded and removed from quarantine, and a made-up path is refused.
#[tokio::test]
async fn a_direct_upload_is_promoted_from_quarantine() {
    let (db, _g) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let (root, _) = sessions(&db).await;

    let ask = |path: &str, body: &str| {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("cookie", format!("{SESSION_COOKIE}={root}"))
            .header("sec-fetch-site", "same-origin")
            .header("x-csrf-token", "tok-123")
            .header("x-requested-with", "fetch")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let response = app
        .clone()
        .oneshot(ask(
            "/admin/media/direct",
            "name=clip.png&content_type=image%2Fpng&size=100",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let plan: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(plan["direct"], false, "a local disk cannot presign");

    // As if a browser had put the bytes where a presigning disk told it to.
    let library = LocalDisk::new(dir.path().join("media"));
    let quarantine = "quarantine/0123456789abcdef0123456789abcdef/clip.png";
    library
        .put(quarantine, stream_of(wide_png(60, 40)))
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(ask(
            "/admin/media/direct/complete",
            &format!("path={quarantine}&name=clip.png&folder="),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let out: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(out["stored"][0]["name"], "clip.png", "{out}");
    let id = out["stored"][0]["id"].as_i64().unwrap();
    let r = record::find(&db, id).await.unwrap().unwrap();
    assert_eq!((r.width, r.height), (Some(60), Some(40)));
    assert!(dir.path().join("media").join(blob_path(&r.hash)).is_file());
    assert!(
        !dir.path().join("media").join(quarantine).exists(),
        "the quarantine copy is gone"
    );

    let response = app
        .clone()
        .oneshot(ask(
            "/admin/media/direct/complete",
            "path=blobs/aa/bb/aabb&name=x.png&folder=",
        ))
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let out: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        out["refused"][0]["name"], "x.png",
        "a path outside quarantine"
    );
}
