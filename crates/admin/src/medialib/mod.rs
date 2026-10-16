//! The media library: a Media tab to browse, upload, arrange and remove the
//! files the application keeps, and the route that serves them.
//!
//! Files live in virtual folders (rows), uploads go to the disk the
//! application registered as `public` (else a local disk under the storage
//! directory), and every file serves at `/storage/media/<hash>/<name>` from
//! the hash, immutable and unguessable. A file still linked from a record is
//! not deleted without `media.delete_linked`; the screen says who uses it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use askama::Template;
use axum::body::Body;
use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use laterite_auth::AuthenticatedUser;
use laterite_core::{t, Module, Text};
use laterite_media::library::{Listing, Sort};
use laterite_media::{
    blob_path, folder, ingest, link, record, stream_of, DiskReg, Kind, LocalDisk, MediaRecord,
    NewMedia, NewMediaExtra, StorageDriver,
};

use crate::html::Markup;
use crate::routes::{PublicRoute, PublicRouteReg, RouteCtx, Screen, ScreenReg};
use crate::session::{FlashLevel, SessionHandle};
use crate::upload::VerifiedUpload;
use crate::{http_cache, AdminError, Permission, Shell, ROLE_ADMIN, ROLE_EDITOR};

/// See the library.
pub const BROWSE: &str = "media.browse";
/// Upload into it.
pub const UPLOAD: &str = "media.upload";
/// Rename, retitle, move, and make or remove folders.
pub const MANAGE: &str = "media.manage";
/// Delete a file nothing links to.
pub const DELETE: &str = "media.delete";
/// Delete a file records still use.
pub const DELETE_LINKED: &str = "media.delete_linked";

/// The disk library uploads go to, by name.
pub const DISK: &str = "public";
/// The most one upload may weigh.
const UPLOAD_LIMIT: u64 = 256 * 1024 * 1024;
const PER_PAGE: u64 = 60;

/// The library module: its tab, its permissions and its serve route.
pub struct MediaLibraryModule;

impl Module for MediaLibraryModule {
    fn id(&self) -> laterite_core::ModuleId {
        laterite_core::ModuleId::new("laterite.medialibrary")
    }
    fn migrations(&self) -> laterite_core::MigrationSet {
        laterite_core::MigrationSet::new("laterite.medialibrary", Vec::new())
    }
    fn admin_base(&self) -> Option<&'static str> {
        Some("/")
    }
    // The admin adds the tab, the permissions and the serve route itself in
    // `router()`, beside its other built-ins, so a test that builds the router
    // directly has them too.
}

/// The library's permissions.
pub(crate) fn permissions() -> Vec<Permission> {
    let group = t!("Media");
    vec![
        Permission::new(BROWSE, t!("Browse the media library"), group.clone())
            .roles([ROLE_ADMIN, ROLE_EDITOR]),
        Permission::new(UPLOAD, t!("Upload media"), group.clone()).roles([ROLE_ADMIN, ROLE_EDITOR]),
        Permission::new(MANAGE, t!("Arrange media and folders"), group.clone())
            .roles([ROLE_ADMIN, ROLE_EDITOR]),
        Permission::new(DELETE, t!("Delete unused media"), group.clone()).roles([ROLE_ADMIN]),
        Permission::new(DELETE_LINKED, t!("Delete media records still use"), group)
            .roles([ROLE_ADMIN]),
    ]
}

/// The Media tab.
pub(crate) fn screen() -> ScreenReg {
    ScreenReg::new("/media", BROWSE, Arc::new(MediaScreen))
        .in_menu(t!("Media"))
        .menu(crate::nav::Placement::new().icon("image").order(80))
}

/// The serve route.
pub(crate) fn serve_route() -> PublicRouteReg {
    PublicRouteReg::new("/storage/media", Arc::new(Serve))
}

/// The disks: each registered one by name, and the library's own.
struct Disks {
    by_name: HashMap<String, (Arc<dyn StorageDriver>, bool)>,
    library: Arc<dyn StorageDriver>,
}

fn disks(ctx: &RouteCtx) -> Disks {
    let mut by_name: HashMap<String, (Arc<dyn StorageDriver>, bool)> = ctx
        .contributions::<DiskReg>()
        .into_iter()
        .map(|d| (d.name.clone(), (Arc::clone(&d.driver), d.private)))
        .collect();
    let library = match by_name.get(DISK) {
        Some((driver, _)) => Arc::clone(driver),
        None => {
            let driver: Arc<dyn StorageDriver> =
                Arc::new(LocalDisk::new(format!("{}/media", ctx.storage_dir())));
            by_name.insert(DISK.to_string(), (Arc::clone(&driver), false));
            driver
        }
    };
    if !by_name.contains_key(crate::import::DISK) {
        let driver: Arc<dyn StorageDriver> = Arc::new(LocalDisk::new(format!(
            "{}/{}",
            ctx.storage_dir(),
            crate::import::DISK
        )));
        by_name.insert(crate::import::DISK.to_string(), (driver, true));
    }
    Disks { by_name, library }
}

struct AppState {
    ctx: RouteCtx,
    disks: Disks,
}

type App = State<Arc<AppState>>;

/// The Media tab.
pub struct MediaScreen;

impl Screen for MediaScreen {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let state = Arc::new(AppState {
            ctx: ctx.clone(),
            disks: disks(ctx),
        });
        Router::new()
            .route("/", get(index))
            .route("/upload", post(upload))
            .route("/folders", post(create_folder))
            .route("/folders/{id}/rename", post(rename_folder))
            .route("/folders/{id}/move", post(move_folder))
            .route("/folders/{id}/delete", post(delete_folder))
            .route("/{id}", get(file))
            .route("/{id}/update", post(update_file))
            .route("/{id}/move", post(move_file))
            .route("/{id}/delete", post(delete_file))
            .with_state(state)
    }
}

/// The public serve route: `/storage/media/<hash>/<name>`.
pub struct Serve;

impl PublicRoute for Serve {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let state = Arc::new(AppState {
            ctx: ctx.clone(),
            disks: disks(ctx),
        });
        Router::new()
            .route("/{hash}/{*name}", get(serve))
            .with_state(state)
    }
}

/// The URL a file serves at: its hash and a display name, under the serve route.
/// A file on a private disk needs [`signed_file_url`] instead.
pub fn file_url(record: &MediaRecord) -> String {
    format!(
        "/storage/media/{}/{}",
        record.hash,
        safe_name(&record.original_name)
    )
}

/// How long a signed URL the admin hands out stays valid.
pub const SIGNED_TTL: Duration = Duration::from_secs(60 * 60);

/// The URL a file on a private disk serves at for `ttl`: [`file_url`] carrying
/// an expiry and a signature over the hash and the expiry, checked by the
/// serve route. The signature is bound to the hash, so it opens one file.
pub fn signed_file_url(ctx: &RouteCtx, record: &MediaRecord, ttl: Duration) -> String {
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_add(ttl.as_secs());
    let token = ctx.sign(token_message(&record.hash, expires).as_bytes());
    format!("{}?e={expires}&t={token}", file_url(record))
}

fn token_message(hash: &str, expires: u64) -> String {
    format!("media:{hash}:{expires}")
}

/// The URL for `record` as the admin shows it: signed when its disk is private.
fn admin_url(app: &AppState, record: &MediaRecord) -> String {
    match app.disks.by_name.get(&record.disk) {
        Some((_, true)) => signed_file_url(&app.ctx, record, SIGNED_TTL),
        _ => file_url(record),
    }
}

/// A file name fit for a URL and a `Content-Disposition`: ASCII letters,
/// digits, dots, hyphens and underscores, never empty.
pub fn safe_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    while out.contains("-.") || out.contains(".-") {
        out = out.replace("-.", ".").replace(".-", ".");
    }
    let out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    if out.is_empty() {
        "file".to_string()
    } else {
        out
    }
}

/// A content type the browser may run script from, served as a download.
fn active_type(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/svg+xml" | "text/html" | "application/xhtml+xml" | "text/javascript"
    )
}

#[derive(serde::Deserialize, Default)]
struct ServeQuery {
    /// The expiry of a signed URL, seconds since the epoch.
    e: Option<u64>,
    /// The signature of a signed URL.
    t: Option<String>,
}

/// Whether `query` carries a live signature for `hash`.
fn token_valid(ctx: &RouteCtx, hash: &str, query: &ServeQuery) -> bool {
    let (Some(expires), Some(token)) = (query.e, query.t.as_deref()) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX);
    expires > now && ctx.verify_signature(token_message(hash, expires).as_bytes(), token)
}

async fn serve(
    State(app): App,
    headers: HeaderMap,
    Path((hash, _name)): Path<(String, String)>,
    Query(query): Query<ServeQuery>,
) -> Response {
    let records = match record::find_all_by_hash(app.ctx.db(), &hash).await {
        Ok(r) => r,
        Err(_) => return AdminError::NotFound.into_response(),
    };
    // A hash may be stored on several disks. A public one serves it to anyone,
    // cacheable for good; a private one only behind a live signature, and then
    // never cached.
    let on = |private: bool| {
        records.iter().find_map(|r| {
            app.disks
                .by_name
                .get(&r.disk)
                .filter(|(_, p)| *p == private)
                .map(|(d, _)| (r, Arc::clone(d)))
        })
    };
    let (record, driver, private) = match on(false) {
        Some((r, d)) => (r, d, false),
        None => match on(true) {
            Some((r, d)) if token_valid(&app.ctx, &hash, &query) => (r, d, true),
            _ => return AdminError::NotFound.into_response(),
        },
    };
    let cache_control = if private {
        "private, no-store".to_string()
    } else {
        http_cache::IMMUTABLE.to_string()
    };
    let etag = format!("\"{}\"", record.hash);
    if !private && http_cache::matches_etag(&headers, &record.hash) {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, etag), (header::CACHE_CONTROL, cache_control)],
        )
            .into_response();
    }
    let stream = match driver.get(&blob_path(&record.hash)).await {
        Ok(s) => s,
        Err(_) => return AdminError::NotFound.into_response(),
    };
    let disposition = if active_type(&record.content_type) || record.kind() == Kind::Other {
        format!(
            "attachment; filename=\"{}\"",
            safe_name(&record.original_name)
        )
    } else {
        format!("inline; filename=\"{}\"", safe_name(&record.original_name))
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, record.content_type.clone()),
            (header::CONTENT_LENGTH, record.size.to_string()),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, cache_control),
            (header::CONTENT_DISPOSITION, disposition),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

// The screen.

#[derive(serde::Deserialize, Default)]
struct BrowseQuery {
    folder: Option<i64>,
    q: Option<String>,
    kind: Option<String>,
    sort: Option<String>,
    page: Option<u64>,
}

struct FolderView {
    name: String,
    href: String,
}

struct FileView {
    name: String,
    kind: &'static str,
    content_type: String,
    size: String,
    url: String,
    href: String,
    is_image: bool,
}

struct CrumbView {
    name: String,
    href: String,
}

#[derive(Template)]
#[template(path = "media.html")]
struct IndexTemplate {
    shell: Shell,
    base: String,
    folder: Option<i64>,
    crumbs: Vec<CrumbView>,
    folders: Vec<FolderView>,
    files: Vec<FileView>,
    total: i64,
    page: u64,
    pages: u64,
    q: String,
    kind: String,
    sort: String,
    kinds: Vec<(&'static str, String)>,
    may_upload: bool,
    may_manage: bool,
    prev_href: Option<String>,
    next_href: Option<String>,
}

fn human_size(bytes: i64) -> String {
    let b = bytes as f64;
    if b < 1024.0 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GB", b / 1024.0 / 1024.0 / 1024.0)
    }
}

fn kind_label(shell: &Shell, kind: Kind) -> String {
    match kind {
        Kind::Image => shell.t("Images"),
        Kind::Video => shell.t("Video"),
        Kind::Audio => shell.t("Audio"),
        Kind::Document => shell.t("Documents"),
        Kind::Other => shell.t("Other"),
    }
}

fn browse_href(
    base: &str,
    folder: Option<i64>,
    q: &str,
    kind: &str,
    sort: &str,
    page: u64,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(f) = folder {
        parts.push(format!("folder={f}"));
    }
    if !q.is_empty() {
        parts.push(format!("q={}", urlencoding(q)));
    }
    if !kind.is_empty() {
        parts.push(format!("kind={kind}"));
    }
    if !sort.is_empty() && sort != "newest" {
        parts.push(format!("sort={sort}"));
    }
    if page > 1 {
        parts.push(format!("page={page}"));
    }
    if parts.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", parts.join("&"))
    }
}

fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn file_view(app: &AppState, r: &MediaRecord) -> FileView {
    FileView {
        name: r.display_name().to_string(),
        kind: r.kind().as_str(),
        content_type: r.content_type.clone(),
        size: human_size(r.size),
        url: admin_url(app, r),
        href: app.ctx.url(&r.id.to_string()),
        is_image: r.kind() == Kind::Image && r.content_type != "image/svg+xml",
    }
}

fn page_of<T: Template>(shell: &Shell, title: Text, template: &T) -> Response {
    match Markup::from_template(template) {
        Ok(body) => shell.page(&title, body),
        Err(e) => {
            tracing::error!(error = %e, "rendering a media page failed");
            AdminError::Internal(anyhow::anyhow!(e)).into_response()
        }
    }
}

async fn index(
    State(app): App,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<BrowseQuery>,
) -> Response {
    let ctx = &app.ctx;
    let db = ctx.db();
    let q = query.q.unwrap_or_default().trim().to_string();
    let kind_name = query.kind.unwrap_or_default();
    let kind = Kind::parse(&kind_name);
    let sort_name = query.sort.unwrap_or_default();
    let sort = Sort::parse(&sort_name).unwrap_or_default();
    let page = query.page.unwrap_or(1).max(1);
    let folder_id = query.folder;
    let base = ctx.url("");

    let crumbs: Vec<CrumbView> = match folder_id {
        Some(id) => folder::ancestors(db, id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|f| CrumbView {
                name: f.name,
                href: browse_href(&base, Some(f.id), "", "", "", 1),
            })
            .collect(),
        None => Vec::new(),
    };
    let folders: Vec<FolderView> = if q.is_empty() {
        folder::children(db, folder_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|f| FolderView {
                href: browse_href(&base, Some(f.id), "", "", "", 1),
                name: f.name,
            })
            .collect()
    } else {
        Vec::new()
    };
    let listing = Listing {
        folder: folder_id,
        search: (!q.is_empty()).then(|| q.clone()),
        kind,
        sort,
        page,
        per_page: PER_PAGE,
    };
    let listed = laterite_media::library::list(db, &listing).await.unwrap_or(
        laterite_media::library::Page {
            items: Vec::new(),
            total: 0,
        },
    );
    let pages = (listed.total as u64).max(1).div_ceil(PER_PAGE);
    let files = listed.items.iter().map(|r| file_view(&app, r)).collect();
    page_of(
        &shell,
        t!("Media"),
        &IndexTemplate {
            prev_href: (page > 1)
                .then(|| browse_href(&base, folder_id, &q, &kind_name, &sort_name, page - 1)),
            next_href: (page < pages)
                .then(|| browse_href(&base, folder_id, &q, &kind_name, &sort_name, page + 1)),
            kinds: Kind::ALL
                .into_iter()
                .map(|k| (k.as_str(), kind_label(&shell, k)))
                .collect(),
            may_upload: user.permissions.allows(UPLOAD),
            may_manage: user.permissions.allows(MANAGE),
            shell: shell.clone(),
            base,
            folder: folder_id,
            crumbs,
            folders,
            files,
            total: listed.total,
            page,
            pages,
            q,
            kind: kind_name,
            sort: sort.as_str().to_string(),
        },
    )
}

fn back_to(ctx: &RouteCtx, folder: Option<i64>) -> Response {
    let base = ctx.url("");
    Redirect::to(&browse_href(&base, folder, "", "", "", 1)).into_response()
}

fn forbidden() -> Response {
    AdminError::Forbidden.into_response()
}

async fn upload(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    mut upload: VerifiedUpload,
) -> Response {
    if !user.permissions.allows(UPLOAD) {
        return forbidden();
    }
    let ctx = &app.ctx;
    let mut folder_id: Option<i64> = None;
    let mut stored = 0usize;
    let mut refused: Vec<String> = Vec::new();
    while let Ok(Some(mut field)) = upload.next_field().await {
        match field.name().unwrap_or_default() {
            "folder" => {
                folder_id = field
                    .text()
                    .await
                    .ok()
                    .and_then(|t| t.trim().parse::<i64>().ok());
            }
            "file" => {
                let name = field
                    .file_name()
                    .map(|n| n.to_string())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "upload".to_string());
                let mut bytes: Vec<u8> = Vec::new();
                let mut too_large = false;
                while let Ok(Some(chunk)) = field.chunk().await {
                    if bytes.len() as u64 + chunk.len() as u64 > UPLOAD_LIMIT {
                        too_large = true;
                        break;
                    }
                    bytes.extend_from_slice(&chunk);
                }
                if too_large {
                    refused.push(name);
                    continue;
                }
                if bytes.is_empty() {
                    continue;
                }
                let ingested = match ingest(
                    app.disks.library.as_ref(),
                    stream_of(bytes),
                    UPLOAD_LIMIT,
                )
                .await
                {
                    Ok(i) => i,
                    Err(e) => {
                        tracing::error!(error = %e, "a library upload could not be stored");
                        refused.push(name);
                        continue;
                    }
                };
                let id = match record::create_with(
                    ctx.db(),
                    NewMedia {
                        disk: DISK.to_string(),
                        hash: ingested.hash,
                        size: ingested.size as i64,
                        content_type: ingested.content_type,
                        original_name: name.clone(),
                        in_library: true,
                        created_by: Some(user.user.id),
                    },
                    NewMediaExtra {
                        folder_id,
                        ..Default::default()
                    },
                )
                .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::error!(error = %e, "a library record could not be written");
                        refused.push(name);
                        continue;
                    }
                };
                ctx.audit(
                    &user,
                    "media.upload",
                    Some("media"),
                    Some(&id.to_string()),
                    Some(&name),
                    None,
                )
                .await;
                stored += 1;
            }
            _ => {}
        }
    }
    if stored > 0 {
        session.push_flash(
            FlashLevel::Success,
            t!("Uploaded {n} file(s).", n = stored as i64),
        );
    }
    if !refused.is_empty() {
        session.push_flash(
            FlashLevel::Error,
            Text::dynamic(format!(
                "Could not store: {} (too large or unreadable).",
                refused.join(", ")
            )),
        );
    }
    if stored == 0 && refused.is_empty() {
        session.push_flash(FlashLevel::Error, t!("Choose a file to upload."));
    }
    back_to(ctx, folder_id)
}

/// An optional folder id as a form sends it: blank for the root.
fn folder_param(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok()
}

#[derive(serde::Deserialize)]
struct FolderForm {
    #[serde(default)]
    parent: String,
    #[serde(default)]
    name: String,
}

fn flash_folder_error(session: &SessionHandle, e: &laterite_media::FolderError) {
    session.push_flash(FlashLevel::Error, Text::dynamic(e.to_string()));
}

async fn create_folder(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Form(form): Form<FolderForm>,
) -> Response {
    if !user.permissions.allows(MANAGE) {
        return forbidden();
    }
    let ctx = &app.ctx;
    let parent = folder_param(&form.parent);
    match folder::create(ctx.db(), parent, &form.name).await {
        Ok(f) => {
            ctx.audit(
                &user,
                "media.folder.create",
                Some("media_folder"),
                Some(&f.id.to_string()),
                Some(&f.path),
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Folder created."));
            back_to(ctx, Some(f.id))
        }
        Err(e) => {
            flash_folder_error(&session, &e);
            back_to(ctx, parent)
        }
    }
}

#[derive(serde::Deserialize)]
struct NameForm {
    name: String,
}

async fn rename_folder(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
    Form(form): Form<NameForm>,
) -> Response {
    if !user.permissions.allows(MANAGE) {
        return forbidden();
    }
    let ctx = &app.ctx;
    match folder::rename(ctx.db(), id, &form.name).await {
        Ok(f) => {
            ctx.audit(
                &user,
                "media.folder.rename",
                Some("media_folder"),
                Some(&id.to_string()),
                Some(&f.path),
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Folder renamed."));
            back_to(ctx, f.parent_id)
        }
        Err(e) => {
            flash_folder_error(&session, &e);
            back_to(ctx, Some(id))
        }
    }
}

#[derive(serde::Deserialize)]
struct MoveForm {
    #[serde(default)]
    to: String,
}

async fn move_folder(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
    Form(form): Form<MoveForm>,
) -> Response {
    if !user.permissions.allows(MANAGE) {
        return forbidden();
    }
    let ctx = &app.ctx;
    let to = folder_param(&form.to);
    match folder::move_to(ctx.db(), id, to).await {
        Ok(f) => {
            ctx.audit(
                &user,
                "media.folder.move",
                Some("media_folder"),
                Some(&id.to_string()),
                Some(&f.path),
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Folder moved."));
            back_to(ctx, to)
        }
        Err(e) => {
            flash_folder_error(&session, &e);
            back_to(ctx, Some(id))
        }
    }
}

async fn delete_folder(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
) -> Response {
    if !user.permissions.allows(MANAGE) {
        return forbidden();
    }
    let ctx = &app.ctx;
    let parent = folder::find(ctx.db(), id)
        .await
        .ok()
        .flatten()
        .and_then(|f| f.parent_id);
    match folder::delete(ctx.db(), id).await {
        Ok(()) => {
            ctx.audit(
                &user,
                "media.folder.delete",
                Some("media_folder"),
                Some(&id.to_string()),
                None,
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Folder deleted."));
            back_to(ctx, parent)
        }
        Err(e) => {
            flash_folder_error(&session, &e);
            back_to(ctx, Some(id))
        }
    }
}

struct UserView {
    owner_type: String,
    owner_id: String,
    field: String,
}

struct FolderChoice {
    id: String,
    path: String,
}

#[derive(Template)]
#[template(path = "media_file.html")]
struct FileTemplate {
    shell: Shell,
    back_href: String,
    file_href: String,
    file: FileView,
    original_name: String,
    title: String,
    alt: String,
    dimensions: String,
    uploaded: String,
    disk: String,
    hash: String,
    users: Vec<UserView>,
    folders: Vec<FolderChoice>,
    folder: String,
    may_manage: bool,
    may_delete: bool,
    may_delete_linked: bool,
}

async fn load(ctx: &RouteCtx, id: i64) -> Option<MediaRecord> {
    record::find(ctx.db(), id).await.ok().flatten()
}

async fn file(
    State(app): App,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
) -> Response {
    let ctx = &app.ctx;
    let Some(r) = load(ctx, id).await else {
        return AdminError::NotFound.into_response();
    };
    let users = link::users_of(ctx.db(), id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|l| UserView {
            owner_type: l.owner_type,
            owner_id: l.owner_id,
            field: l.field,
        })
        .collect();
    let folders: Vec<FolderChoice> = folder::all(ctx.db())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|f| FolderChoice {
            id: f.id.to_string(),
            path: f.path,
        })
        .collect();
    let dimensions = match (r.width, r.height) {
        (Some(w), Some(h)) => format!("{w} × {h}"),
        _ => String::new(),
    };
    let view = file_view(&app, &r);
    page_of(
        &shell,
        Text::dynamic(r.display_name().to_string()),
        &FileTemplate {
            back_href: browse_href(&ctx.url(""), r.folder_id, "", "", "", 1),
            file_href: ctx.url(&id.to_string()),
            file: view,
            original_name: r.original_name.clone(),
            title: r.title.clone().unwrap_or_default(),
            alt: r.alt.clone().unwrap_or_default(),
            dimensions,
            uploaded: shell.datetime(&r.created_at),
            disk: r.disk.clone(),
            hash: r.hash.clone(),
            users,
            folders,
            folder: r.folder_id.map(|f| f.to_string()).unwrap_or_default(),
            may_manage: user.permissions.allows(MANAGE),
            may_delete: user.permissions.allows(DELETE),
            may_delete_linked: user.permissions.allows(DELETE_LINKED),
            shell: shell.clone(),
        },
    )
}

#[derive(serde::Deserialize)]
struct DetailsForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    alt: String,
}

async fn update_file(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
    Form(form): Form<DetailsForm>,
) -> Response {
    if !user.permissions.allows(MANAGE) {
        return forbidden();
    }
    let ctx = &app.ctx;
    let here = ctx.url(&id.to_string());
    let Some(r) = load(ctx, id).await else {
        return AdminError::NotFound.into_response();
    };
    let name = form.name.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.len() > 255 {
        session.push_flash(
            FlashLevel::Error,
            t!("Give the file a name without slashes."),
        );
        return Redirect::to(&here).into_response();
    }
    let blank = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    let title = blank(&form.title);
    let alt = blank(&form.alt);
    let renamed = if name != r.original_name {
        record::rename(ctx.db(), id, name).await
    } else {
        Ok(())
    };
    match renamed.and(record::update_details(ctx.db(), id, title.as_deref(), alt.as_deref()).await)
    {
        Ok(()) => {
            ctx.audit(
                &user,
                "media.update",
                Some("media"),
                Some(&id.to_string()),
                Some(name),
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("File saved."));
        }
        Err(e) => {
            tracing::error!(error = %e, "a media record could not be saved");
            session.push_flash(FlashLevel::Error, t!("Could not save. Please try again."));
        }
    }
    Redirect::to(&here).into_response()
}

async fn move_file(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
    Form(form): Form<MoveForm>,
) -> Response {
    if !user.permissions.allows(MANAGE) {
        return forbidden();
    }
    let ctx = &app.ctx;
    let here = ctx.url(&id.to_string());
    let to = folder_param(&form.to);
    if let Some(to) = to {
        if folder::find(ctx.db(), to).await.ok().flatten().is_none() {
            session.push_flash(FlashLevel::Error, t!("No such folder."));
            return Redirect::to(&here).into_response();
        }
    }
    match record::move_to_folder(ctx.db(), id, to).await {
        Ok(()) => {
            ctx.audit(
                &user,
                "media.move",
                Some("media"),
                Some(&id.to_string()),
                None,
                Some(&format!(
                    "{{\"folder\":{}}}",
                    to.map(|f| f.to_string()).unwrap_or("null".into())
                )),
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("File moved."));
        }
        Err(e) => {
            tracing::error!(error = %e, "a media record could not be moved");
            session.push_flash(FlashLevel::Error, t!("Could not save. Please try again."));
        }
    }
    Redirect::to(&here).into_response()
}

async fn delete_file(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
) -> Response {
    let ctx = &app.ctx;
    let here = ctx.url(&id.to_string());
    let Some(r) = load(ctx, id).await else {
        return AdminError::NotFound.into_response();
    };
    let users = link::users_of(ctx.db(), id).await.unwrap_or_default();
    let needed = if users.is_empty() {
        DELETE
    } else {
        DELETE_LINKED
    };
    if !user.permissions.allows(needed) {
        if users.is_empty() {
            return forbidden();
        }
        session.push_flash(
            FlashLevel::Error,
            t!(
                "{n} record(s) still use this file; deleting it needs the permission to delete linked media.",
                n = users.len() as i64
            ),
        );
        return Redirect::to(&here).into_response();
    }
    let folder_id = r.folder_id;
    match record::delete(ctx.db(), id).await {
        Ok(last) => {
            if last {
                if let Some((driver, _)) = app.disks.by_name.get(&r.disk) {
                    if let Err(e) = driver.delete(&blob_path(&r.hash)).await {
                        tracing::warn!(error = %e, hash = %r.hash, "a blob could not be removed");
                    }
                }
            }
            ctx.audit(
                &user,
                "media.delete",
                Some("media"),
                Some(&id.to_string()),
                Some(&r.original_name),
                Some(&format!("{{\"links\":{}}}", users.len())),
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("File deleted."));
            back_to(ctx, folder_id)
        }
        Err(e) => {
            tracing::error!(error = %e, "a media record could not be deleted");
            session.push_flash(FlashLevel::Error, t!("Could not delete. Please try again."));
            Redirect::to(&here).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe_for_a_url() {
        assert_eq!(
            safe_name("My Photo (1).JPG"),
            "My-Photo-1-.JPG".replace("-.", ".")
        );
        assert_eq!(safe_name("../../etc/passwd"), "etc-passwd");
        assert_eq!(safe_name("***"), "file");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }
}
