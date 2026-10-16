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
use laterite_media::preset::{self, Preset};
use laterite_media::{
    blob_path, folder, ingest, link, record, stream_of, variant, DiskReg, Kind, LocalDisk,
    MediaRecord, NewMedia, NewMediaExtra, StorageDriver, Variants,
};

use crate::html::Markup;
use crate::routes::{PublicRoute, PublicRouteReg, RouteCtx, Screen, ScreenReg};

pub mod field;
use crate::session::{FlashLevel, SessionHandle};
use crate::upload::VerifiedUpload;
use crate::{http_cache, AdminError, PageHead, Permission, Shell, ROLE_ADMIN, ROLE_EDITOR};

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
    /// Every preset a variant may be made in, by name.
    presets: HashMap<String, Preset>,
    variants: Variants,
}

impl AppState {
    fn new(ctx: &RouteCtx) -> Arc<Self> {
        Arc::new(Self {
            ctx: ctx.clone(),
            disks: disks(ctx),
            presets: preset::all(ctx.contributions::<Preset>().into_iter().cloned()),
            variants: Variants::new(),
        })
    }
}

type App = State<Arc<AppState>>;

/// The Media tab.
pub struct MediaScreen;

impl Screen for MediaScreen {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let state = AppState::new(ctx);
        Router::new()
            .route("/", get(index))
            .route("/pick", get(pick))
            .route("/resolve", get(resolve))
            .route("/upload", post(upload))
            .route("/direct", post(direct_start))
            .route("/direct/complete", post(direct_complete))
            .route("/folders", post(create_folder))
            .route("/folders/{id}/rename", post(rename_folder))
            .route("/folders/{id}/move", post(move_folder))
            .route("/folders/{id}/delete", post(delete_folder))
            .route("/{id}", get(file))
            .route("/{id}/update", post(update_file))
            .route("/{id}/move", post(move_file))
            .route("/{id}/crop", post(crop_file))
            .route("/{id}/delete", post(delete_file))
            .with_state(state)
    }
}

/// The public serve route: `/storage/media/<hash>/<name>`.
pub struct Serve;

impl PublicRoute for Serve {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let state = AppState::new(ctx);
        Router::new()
            .route("/{hash}/{*name}", get(serve))
            .with_state(state)
    }
}

/// The URL a file serves at: its hash and a display name, under the serve route.
/// A file on a private disk needs [`signed_file_url`] instead.
pub fn file_url(record: &MediaRecord) -> String {
    url_for(record, None)
}

/// The URL a file's variant in `preset` serves at: the preset sits between
/// the hash and the name. An unknown preset is a 404; a file that is not an
/// image serves as it is.
pub fn variant_url(record: &MediaRecord, preset: &str) -> String {
    url_for(record, Some(preset))
}

fn url_for(record: &MediaRecord, preset: Option<&str>) -> String {
    match preset {
        Some(p) => format!(
            "/storage/media/{}/{p}/{}",
            record.hash,
            safe_name(&record.original_name)
        ),
        None => format!(
            "/storage/media/{}/{}",
            record.hash,
            safe_name(&record.original_name)
        ),
    }
}

/// How long a signed URL the admin hands out stays valid.
pub const SIGNED_TTL: Duration = Duration::from_secs(60 * 60);

/// The URL a file on a private disk serves at for `ttl`: [`file_url`] carrying
/// an expiry and a signature over the hash and the expiry, checked by the
/// serve route. The signature is bound to the hash, so it opens one file.
pub fn signed_file_url(ctx: &RouteCtx, record: &MediaRecord, ttl: Duration) -> String {
    signed_url_for(ctx, record, None, ttl)
}

/// [`variant_url`] with the signature a private disk needs; the token is the
/// file's, so one signature opens every variant of it.
pub fn signed_variant_url(
    ctx: &RouteCtx,
    record: &MediaRecord,
    preset: &str,
    ttl: Duration,
) -> String {
    signed_url_for(ctx, record, Some(preset), ttl)
}

fn signed_url_for(
    ctx: &RouteCtx,
    record: &MediaRecord,
    preset: Option<&str>,
    ttl: Duration,
) -> String {
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_add(ttl.as_secs());
    let token = ctx.sign(token_message(&record.hash, expires).as_bytes());
    format!("{}?e={expires}&t={token}", url_for(record, preset))
}

fn token_message(hash: &str, expires: u64) -> String {
    format!("media:{hash}:{expires}")
}

/// The URL for `record` as the admin shows it: signed when its disk is private.
fn admin_url(app: &AppState, record: &MediaRecord) -> String {
    admin_url_for(app, record, None)
}

fn admin_url_for(app: &AppState, record: &MediaRecord, preset: Option<&str>) -> String {
    match app.disks.by_name.get(&record.disk) {
        Some((_, true)) => signed_url_for(&app.ctx, record, preset, SIGNED_TTL),
        _ => url_for(record, preset),
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
    Path((hash, name)): Path<(String, String)>,
    Query(query): Query<ServeQuery>,
) -> Response {
    // `<hash>/<name>` is the original; `<hash>/<preset>/<name>` a variant.
    let preset = match name.split_once('/') {
        Some((preset, _)) => match app.presets.get(preset) {
            Some(p) => Some(p),
            None => return AdminError::NotFound.into_response(),
        },
        None => None,
    };
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
    // A variant of an image is made on first request; any other file serves
    // as it is under a preset too, so a template never has to ask first.
    let variant = match preset {
        Some(p) if record.kind() == Kind::Image && record.content_type != "image/svg+xml" => {
            match app
                .variants
                .ensure(app.ctx.db(), driver.as_ref(), record, p)
                .await
            {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, hash = %record.hash, preset = %p.name, "a variant could not be made");
                    return AdminError::NotFound.into_response();
                }
            }
        }
        _ => None,
    };
    let validator = match &variant {
        Some(v) => format!("{}/{}", record.hash, v.preset),
        None => record.hash.clone(),
    };
    let etag = format!("\"{validator}\"");
    if !private && http_cache::matches_etag(&headers, &validator) {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, etag), (header::CACHE_CONTROL, cache_control)],
        )
            .into_response();
    }
    let (path, content_type, size) = match &variant {
        Some(v) => (v.path.clone(), v.content_type.clone(), v.size),
        None => (
            blob_path(&record.hash),
            record.content_type.clone(),
            record.size,
        ),
    };
    let stream = match driver.get(&path).await {
        Ok(s) => s,
        Err(_) => return AdminError::NotFound.into_response(),
    };
    let disposition = if active_type(&content_type) || record.kind() == Kind::Other {
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
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_LENGTH, size.to_string()),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, cache_control),
            (header::CONTENT_DISPOSITION, disposition),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

// The picker: the library as a dialog fragment for the `media` field, and
// the lookup its tiles are drawn from.

#[derive(serde::Deserialize, Default)]
struct PickQuery {
    folder: Option<i64>,
    q: Option<String>,
    /// `image` narrows to images; anything else offers every file.
    mode: Option<String>,
    page: Option<u64>,
}

struct PickFile {
    id: i64,
    name: String,
    kind: &'static str,
    size: String,
    thumb: String,
    url: String,
    is_image: bool,
}

#[derive(Template)]
#[template(path = "media_pick.html")]
struct PickTemplate {
    shell: Shell,
    pick_url: String,
    mode: String,
    q: String,
    folder: Option<i64>,
    crumbs: Vec<CrumbView>,
    folders: Vec<FolderView>,
    files: Vec<PickFile>,
    page: u64,
    pages: u64,
}

fn pick_href(base: &str, folder: Option<i64>, q: &str, mode: &str, page: u64) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(f) = folder {
        parts.push(format!("folder={f}"));
    }
    if !q.is_empty() {
        parts.push(format!("q={}", urlencoding(q)));
    }
    if !mode.is_empty() {
        parts.push(format!("mode={mode}"));
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

async fn pick(
    State(app): App,
    Extension(shell): Extension<Shell>,
    Query(query): Query<PickQuery>,
) -> Response {
    let ctx = &app.ctx;
    let db = ctx.db();
    let q = query.q.unwrap_or_default().trim().to_string();
    let mode = query.mode.unwrap_or_default();
    let page = query.page.unwrap_or(1).max(1);
    let folder_id = query.folder;
    let pick_url = ctx.url("pick");
    let crumbs: Vec<CrumbView> = match folder_id {
        Some(id) => folder::ancestors(db, id).await.unwrap_or_default(),
        None => Vec::new(),
    }
    .into_iter()
    .map(|f| CrumbView {
        href: pick_href(&pick_url, Some(f.id), "", &mode, 1),
        name: f.name,
    })
    .collect();
    let folders: Vec<FolderView> = if q.is_empty() {
        folder::children(db, folder_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|f| FolderView {
                href: pick_href(&pick_url, Some(f.id), "", &mode, 1),
                name: f.name,
                active: false,
            })
            .collect()
    } else {
        Vec::new()
    };
    let listing = Listing {
        folder: folder_id,
        search: (!q.is_empty()).then(|| q.clone()),
        kind: (mode == "image").then_some(Kind::Image),
        sort: Sort::default(),
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
    let files = listed
        .items
        .iter()
        .map(|r| PickFile {
            id: r.id,
            name: r.display_name().to_string(),
            kind: r.kind().as_str(),
            size: human_size(r.size),
            thumb: admin_url_for(&app, r, Some(preset::THUMB)),
            url: admin_url(&app, r),
            is_image: r.kind() == Kind::Image && r.content_type != "image/svg+xml",
        })
        .collect();
    match Markup::from_template(&PickTemplate {
        shell: shell.clone(),
        pick_url,
        mode,
        q,
        folder: folder_id,
        crumbs,
        folders,
        files,
        page,
        pages,
    }) {
        Ok(body) => axum::response::Html(body.into_string()).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "rendering the media picker failed");
            AdminError::Internal(anyhow::anyhow!(e)).into_response()
        }
    }
}

#[derive(serde::Deserialize, Default)]
struct ResolveQuery {
    #[serde(default)]
    ids: String,
}

/// The tiles for ids a field holds: name, kind, thumbnail and URL per file,
/// in the order asked; an id that no longer exists is left out.
async fn resolve(State(app): App, Query(query): Query<ResolveQuery>) -> Response {
    let mut out = Vec::new();
    for id in query
        .ids
        .split(',')
        .filter_map(|p| p.trim().parse::<i64>().ok())
        .take(64)
    {
        if let Ok(Some(r)) = record::find(app.ctx.db(), id).await {
            out.push(serde_json::json!({
                "id": r.id,
                "name": r.display_name(),
                "kind": r.kind().as_str(),
                "size": human_size(r.size),
                "thumb": admin_url_for(&app, &r, Some(preset::THUMB)),
                "url": admin_url(&app, &r),
                "is_image": r.kind() == Kind::Image && r.content_type != "image/svg+xml",
            }));
        }
    }
    axum::Json(out).into_response()
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
    /// Whether the folder shown holds, or is, the one being browsed.
    active: bool,
}

/// One entry in the browse pane: a kind or a sort order.
struct NavLink {
    label: String,
    href: String,
    icon: &'static str,
    active: bool,
}

/// The controls beside the Media title.
#[derive(Template)]
#[template(path = "media_actions.html")]
struct ActionsTemplate {
    shell: Shell,
    may_upload: bool,
    may_manage: bool,
}

/// The controls beside a file's title.
#[derive(Template)]
#[template(path = "media_file_actions.html")]
struct FileActionsTemplate {
    shell: Shell,
    url: String,
    file_href: String,
    may_delete: bool,
    linked: bool,
    /// Whether the file is an image the operator may crop.
    may_crop: bool,
}

struct FileView {
    name: String,
    kind: &'static str,
    content_type: String,
    size: String,
    url: String,
    /// The `thumb` variant for the grid, the `preview` one for the file page.
    thumb_url: String,
    preview_url: String,
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
    /// The library's root under the current kind and sort.
    library_href: String,
    upload_url: String,
    /// Where the uploader asks for a presigned PUT before sending.
    upload_direct: String,
    /// The fields the uploader sends before each file (`folder=3`).
    upload_fields: String,
    upload_hint: String,
    folder: Option<i64>,
    folder_name: String,
    crumbs: Vec<CrumbView>,
    kind_links: Vec<NavLink>,
    sort_links: Vec<NavLink>,
    root_folders: Vec<FolderView>,
    folders: Vec<FolderView>,
    files: Vec<FileView>,
    total: i64,
    page: u64,
    pages: u64,
    q: String,
    kind: String,
    sort: String,
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

fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "image",
        Kind::Video => "film",
        Kind::Audio => "music",
        Kind::Document => "file-text",
        Kind::Other => "file",
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
        thumb_url: admin_url_for(app, r, Some(preset::THUMB)),
        preview_url: admin_url_for(app, r, Some(preset::PREVIEW)),
        href: app.ctx.url(&r.id.to_string()),
        is_image: r.kind() == Kind::Image && r.content_type != "image/svg+xml",
    }
}

fn page_with<T: Template>(shell: &Shell, head: PageHead, template: &T) -> Response {
    match Markup::from_template(template) {
        Ok(body) => shell.page_with(head, body),
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

    let ancestors = match folder_id {
        Some(id) => folder::ancestors(db, id).await.unwrap_or_default(),
        None => Vec::new(),
    };
    let folder_name = ancestors.last().map(|f| f.name.clone()).unwrap_or_default();
    let top = ancestors.first().map(|f| f.id);
    let crumbs: Vec<CrumbView> = ancestors
        .iter()
        .map(|f| CrumbView {
            name: f.name.clone(),
            href: browse_href(&base, Some(f.id), "", &kind_name, &sort_name, 1),
        })
        .collect();
    let folders: Vec<FolderView> = if q.is_empty() {
        folder::children(db, folder_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|f| FolderView {
                href: browse_href(&base, Some(f.id), "", &kind_name, &sort_name, 1),
                name: f.name,
                active: false,
            })
            .collect()
    } else {
        Vec::new()
    };
    let root_folders: Vec<FolderView> = folder::children(db, None)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|f| FolderView {
            href: browse_href(&base, Some(f.id), "", &kind_name, &sort_name, 1),
            active: top == Some(f.id),
            name: f.name,
        })
        .collect();
    let kind_links = std::iter::once(NavLink {
        label: shell.t("Everything"),
        href: browse_href(&base, folder_id, &q, "", &sort_name, 1),
        icon: "layers",
        active: kind.is_none(),
    })
    .chain(Kind::ALL.into_iter().map(|k| NavLink {
        label: kind_label(&shell, k),
        href: browse_href(&base, folder_id, &q, k.as_str(), &sort_name, 1),
        icon: kind_icon(k),
        active: kind == Some(k),
    }))
    .collect();
    let sort_links = [
        ("newest", shell.t("Newest first")),
        ("oldest", shell.t("Oldest first")),
        ("name", shell.t("Name")),
        ("size", shell.t("Largest first")),
    ]
    .into_iter()
    .map(|(code, label)| NavLink {
        label,
        href: browse_href(&base, folder_id, &q, &kind_name, code, 1),
        icon: "",
        active: Sort::parse(code).unwrap_or_default() == sort,
    })
    .collect();
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
    let may_upload = user.permissions.allows(UPLOAD);
    let may_manage = user.permissions.allows(MANAGE);
    let actions = Markup::from_template(&ActionsTemplate {
        shell: shell.clone(),
        may_upload,
        may_manage,
    })
    .unwrap_or_else(|_| Markup::empty());
    page_with(
        &shell,
        PageHead::new(t!("Media")).actions(actions),
        &IndexTemplate {
            library_href: browse_href(&base, None, "", &kind_name, &sort_name, 1),
            upload_url: format!("{base}/upload"),
            upload_direct: format!("{base}/direct"),
            upload_fields: folder_id.map(|f| format!("folder={f}")).unwrap_or_default(),
            upload_hint: if crumbs.is_empty() {
                shell.t("Into the library. Identical files are stored once.")
            } else {
                shell.t("Into this folder. Identical files are stored once.")
            },
            folder_name,
            kind_links,
            sort_links,
            root_folders,
            prev_href: (page > 1)
                .then(|| browse_href(&base, folder_id, &q, &kind_name, &sort_name, page - 1)),
            next_href: (page < pages)
                .then(|| browse_href(&base, folder_id, &q, &kind_name, &sort_name, page + 1)),
            may_upload,
            may_manage,
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

/// One file the uploader stored, as its JSON answer names it.
#[derive(serde::Serialize)]
struct StoredFile {
    id: i64,
    name: String,
    href: String,
    url: String,
}

/// One file the uploader refused, and why.
#[derive(serde::Serialize)]
struct RefusedFile {
    name: String,
    reason: String,
}

async fn upload(
    State(app): App,
    Extension(shell): Extension<Shell>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
    mut upload: VerifiedUpload,
) -> Response {
    if !user.permissions.allows(UPLOAD) {
        return forbidden();
    }
    // The uploader widget sends one file per request and reads the answer;
    // a plain form submit is redirected back to the folder with a flash.
    let from_script = headers
        .get("x-requested-with")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "fetch");
    let ctx = &app.ctx;
    let mut folder_id: Option<i64> = None;
    let mut stored: Vec<StoredFile> = Vec::new();
    let mut refused: Vec<RefusedFile> = Vec::new();
    let refuse = |name: String, reason: Text| RefusedFile {
        name,
        reason: shell.tt(&reason),
    };
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
                    refused.push(refuse(name, t!("Larger than the upload limit.")));
                    continue;
                }
                if bytes.is_empty() {
                    continue;
                }
                // Metadata goes before the hash, so two copies of one photo
                // with different EXIF still share a blob.
                let prepared = laterite_media::image::prepare(bytes::Bytes::from(bytes));
                let ingested = match ingest(
                    app.disks.library.as_ref(),
                    stream_of(prepared.bytes.clone()),
                    UPLOAD_LIMIT,
                )
                .await
                {
                    Ok(i) => i,
                    Err(e) => {
                        tracing::error!(error = %e, "a library upload could not be stored");
                        refused.push(refuse(name, t!("Could not be stored.")));
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
                        width: prepared.width.map(i64::from),
                        height: prepared.height.map(i64::from),
                        meta: (prepared.orientation != 1)
                            .then(|| serde_json::json!({ "orientation": prepared.orientation })),
                    },
                )
                .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::error!(error = %e, "a library record could not be written");
                        refused.push(refuse(name, t!("Could not be recorded.")));
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
                let url = match record::find(ctx.db(), id).await {
                    Ok(Some(r)) => admin_url(&app, &r),
                    _ => String::new(),
                };
                stored.push(StoredFile {
                    id,
                    name,
                    href: ctx.url(&id.to_string()),
                    url,
                });
            }
            _ => {}
        }
    }
    if from_script {
        return axum::Json(serde_json::json!({ "stored": stored, "refused": refused }))
            .into_response();
    }
    if !stored.is_empty() {
        session.push_flash(
            FlashLevel::Success,
            t!("Uploaded {n} file(s).", n = stored.len() as i64),
        );
    }
    if !refused.is_empty() {
        let names: Vec<&str> = refused.iter().map(|r| r.name.as_str()).collect();
        session.push_flash(
            FlashLevel::Error,
            Text::dynamic(format!(
                "Could not store: {} (too large or unreadable).",
                names.join(", ")
            )),
        );
    }
    if stored.is_empty() && refused.is_empty() {
        session.push_flash(FlashLevel::Error, t!("Choose a file to upload."));
    }
    back_to(ctx, folder_id)
}

/// How long a browser has to finish a direct upload once it asked.
const DIRECT_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(serde::Deserialize)]
struct DirectStartForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    content_type: String,
    #[serde(default)]
    size: u64,
}

/// The uploader asks before sending: a disk that presigns answers with a PUT
/// URL under a quarantine prefix the server chose; any other answers that the
/// file goes through the application.
async fn direct_start(
    State(app): App,
    Extension(user): Extension<AuthenticatedUser>,
    Form(form): Form<DirectStartForm>,
) -> Response {
    if !user.permissions.allows(UPLOAD) {
        return forbidden();
    }
    if form.size > UPLOAD_LIMIT {
        return axum::Json(serde_json::json!({ "direct": false, "reason": "too large" }))
            .into_response();
    }
    let content_type = if form.content_type.is_empty() {
        "application/octet-stream".to_string()
    } else {
        form.content_type.chars().take(120).collect()
    };
    let path = format!("quarantine/{}/{}", quarantine_id(), safe_name(&form.name));
    match app
        .disks
        .library
        .presigned_put(&path, &content_type, DIRECT_TTL)
        .await
    {
        Ok(Some(url)) => axum::Json(serde_json::json!({
            "direct": true,
            "url": url,
            "path": path,
            "headers": { "Content-Type": content_type },
        }))
        .into_response(),
        Ok(None) => axum::Json(serde_json::json!({ "direct": false })).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "a direct upload could not be presigned");
            axum::Json(serde_json::json!({ "direct": false })).into_response()
        }
    }
}

/// Thirty-two hex characters nobody can guess, from the signing key's source
/// of randomness.
fn quarantine_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(serde::Deserialize)]
struct DirectCompleteForm {
    #[serde(default)]
    path: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    folder: String,
}

/// The browser finished its PUT: the server reads the quarantined object,
/// sniffs and hashes it, strips an image's metadata, stores it under its hash,
/// records it and removes the quarantine copy. The client never names a blob
/// path; it only ever hands back the quarantine path it was given.
async fn direct_complete(
    State(app): App,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Form(form): Form<DirectCompleteForm>,
) -> Response {
    if !user.permissions.allows(UPLOAD) {
        return forbidden();
    }
    let ctx = &app.ctx;
    let refused = |reason: Text| {
        axum::Json(serde_json::json!({
            "stored": [],
            "refused": [{ "name": form.name.clone(), "reason": shell.tt(&reason) }],
        }))
        .into_response()
    };
    let path = form.path.trim();
    let well_formed = path.starts_with("quarantine/")
        && path.len() < 300
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'))
        && !path.contains("..");
    if !well_formed {
        return refused(t!("The upload could not be found."));
    }
    let disk = Arc::clone(&app.disks.library);
    let size = match disk.metadata(path).await {
        Ok(Some(meta)) => meta.size,
        _ => return refused(t!("The upload could not be found.")),
    };
    if size == 0 || size > UPLOAD_LIMIT {
        let _ = disk.delete(path).await;
        return refused(t!("Larger than the upload limit."));
    }
    let mut stream = match disk.get(path).await {
        Ok(s) => s,
        Err(_) => return refused(t!("The upload could not be found.")),
    };
    use futures::TryStreamExt;
    let mut bytes: Vec<u8> = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    while let Ok(Some(chunk)) = stream.try_next().await {
        bytes.extend_from_slice(&chunk);
    }
    let prepared = laterite_media::image::prepare(bytes::Bytes::from(bytes));
    let ingested = match ingest(
        disk.as_ref(),
        stream_of(prepared.bytes.clone()),
        UPLOAD_LIMIT,
    )
    .await
    {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(error = %e, "a direct upload could not be stored");
            return refused(t!("Could not be stored."));
        }
    };
    let _ = disk.delete(path).await;
    let folder_id = folder_param(&form.folder);
    let name = if form.name.trim().is_empty() {
        "upload".to_string()
    } else {
        form.name.trim().to_string()
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
            width: prepared.width.map(i64::from),
            height: prepared.height.map(i64::from),
            meta: (prepared.orientation != 1)
                .then(|| serde_json::json!({ "orientation": prepared.orientation })),
        },
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "a direct upload could not be recorded");
            return refused(t!("Could not be recorded."));
        }
    };
    ctx.audit(
        &user,
        "media.upload",
        Some("media"),
        Some(&id.to_string()),
        Some(&name),
        Some("{\"direct\":true}"),
    )
    .await;
    let url = match record::find(ctx.db(), id).await {
        Ok(Some(r)) => admin_url(&app, &r),
        _ => String::new(),
    };
    axum::Json(serde_json::json!({
        "stored": [{ "id": id, "name": name, "href": ctx.url(&id.to_string()), "url": url }],
        "refused": [],
    }))
    .into_response()
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
    file_href: String,
    /// The image's size, for the crop dialog; zero for anything else.
    width: i64,
    height: i64,
    may_crop: bool,
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
    let Some(mut r) = load(ctx, id).await else {
        return AdminError::NotFound.into_response();
    };
    // An image stored before sizes were recorded is measured the first time
    // its page opens, so the crop dialog and the templates have its size.
    if r.width.is_none() && r.kind() == Kind::Image && r.content_type != "image/svg+xml" {
        let measured = measure(&app, &r).await;
        if let Some((w, h)) = measured {
            let _ = record::set_dimensions(ctx.db(), id, Some(w), Some(h)).await;
            r.width = Some(w);
            r.height = Some(h);
        }
    }
    let users: Vec<UserView> = link::users_of(ctx.db(), id)
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
    let folder_label = ancestors_name(ctx, r.folder_id).await;
    let linked = !users.is_empty();
    let may_delete = if linked {
        user.permissions.allows(DELETE_LINKED)
    } else {
        user.permissions.allows(DELETE)
    };
    let may_crop = user.permissions.allows(MANAGE) && view.is_image;
    let actions = Markup::from_template(&FileActionsTemplate {
        shell: shell.clone(),
        url: view.url.clone(),
        file_href: ctx.url(&id.to_string()),
        may_delete,
        linked,
        may_crop,
    })
    .unwrap_or_else(|_| Markup::empty());
    page_with(
        &shell,
        PageHead::new(Text::dynamic(r.display_name().to_string()))
            .back(
                if folder_label.is_empty() {
                    t!("Media")
                } else {
                    Text::dynamic(folder_label)
                },
                browse_href(&ctx.url(""), r.folder_id, "", "", "", 1),
            )
            .actions(actions),
        &FileTemplate {
            file_href: ctx.url(&id.to_string()),
            width: r.width.unwrap_or(0),
            height: r.height.unwrap_or(0),
            may_crop,
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
            shell: shell.clone(),
        },
    )
}

/// An image's size read from its blob's header.
async fn measure(app: &AppState, r: &MediaRecord) -> Option<(i64, i64)> {
    use futures::TryStreamExt;
    let driver = Arc::clone(&app.disks.by_name.get(&r.disk)?.0);
    let path = blob_path(&r.hash);
    let mut stream = driver.get(&path).await.ok()?;
    let mut bytes: Vec<u8> = Vec::new();
    while let Ok(Some(chunk)) = stream.try_next().await {
        bytes.extend_from_slice(&chunk);
        // Every header this reader understands sits in the first kilobytes,
        // except a JPEG whose metadata comes first; a megabyte covers both.
        if bytes.len() > 1024 * 1024 {
            break;
        }
    }
    let (w, h) = laterite_media::image::header_dimensions(&bytes)?;
    Some((i64::from(w), i64::from(h)))
}

/// The name of the folder a file sits in, for the way back; empty at the root.
async fn ancestors_name(ctx: &RouteCtx, folder_id: Option<i64>) -> String {
    match folder_id {
        Some(id) => folder::find(ctx.db(), id)
            .await
            .ok()
            .flatten()
            .map(|f| f.name)
            .unwrap_or_default(),
        None => String::new(),
    }
}

#[derive(serde::Deserialize)]
struct CropForm {
    #[serde(default)]
    x: u32,
    #[serde(default)]
    y: u32,
    #[serde(default)]
    w: u32,
    #[serde(default)]
    h: u32,
    /// `new` (the default) stores the crop as another file beside the
    /// original; `replace` rewrites this file's bytes.
    #[serde(default)]
    mode: String,
}

/// Crops an image to the rectangle the operator drew, as a new file in the
/// same folder or in place of the original.
async fn crop_file(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
    Form(form): Form<CropForm>,
) -> Response {
    if !user.permissions.allows(MANAGE) {
        return forbidden();
    }
    let ctx = &app.ctx;
    let here = ctx.url(&id.to_string());
    let Some(r) = load(ctx, id).await else {
        return AdminError::NotFound.into_response();
    };
    if r.kind() != Kind::Image || r.content_type == "image/svg+xml" {
        session.push_flash(FlashLevel::Error, t!("Only an image can be cropped."));
        return Redirect::to(&here).into_response();
    }
    if form.w == 0 || form.h == 0 {
        session.push_flash(FlashLevel::Error, t!("Draw the area to keep first."));
        return Redirect::to(&here).into_response();
    }
    let Some((driver, _)) = app.disks.by_name.get(&r.disk) else {
        session.push_flash(FlashLevel::Error, t!("This file's disk is not registered."));
        return Redirect::to(&here).into_response();
    };
    let mut bytes: Vec<u8> = Vec::with_capacity(usize::try_from(r.size).unwrap_or(0));
    let mut stream = match driver.get(&blob_path(&r.hash)).await {
        Ok(s) => s,
        Err(_) => return AdminError::NotFound.into_response(),
    };
    use futures::TryStreamExt;
    while let Ok(Some(chunk)) = stream.try_next().await {
        bytes.extend_from_slice(&chunk);
    }
    let orientation = r
        .meta
        .get("orientation")
        .and_then(|o| o.as_u64())
        .and_then(|o| u8::try_from(o).ok())
        .unwrap_or(1);
    let rect = laterite_media::image::Rect {
        x: form.x,
        y: form.y,
        width: form.w,
        height: form.h,
    };
    let rendered = match tokio::task::spawn_blocking(move || {
        laterite_media::image::crop(&bytes, rect, orientation)
    })
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            session.push_flash(FlashLevel::Error, Text::dynamic(e.to_string()));
            return Redirect::to(&here).into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "a crop task failed");
            return AdminError::Internal(anyhow::anyhow!(e)).into_response();
        }
    };
    let ingested = match ingest(
        driver.as_ref(),
        stream_of(rendered.bytes.clone()),
        UPLOAD_LIMIT,
    )
    .await
    {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(error = %e, "a crop could not be stored");
            session.push_flash(FlashLevel::Error, t!("The crop could not be stored."));
            return Redirect::to(&here).into_response();
        }
    };
    let (width, height) = (
        Some(i64::from(rendered.width)),
        Some(i64::from(rendered.height)),
    );
    if form.mode == "replace" {
        let variants = variant::list_for(ctx.db(), id).await.unwrap_or_default();
        let _ = variant::delete_for(ctx.db(), id).await;
        for v in &variants {
            let _ = driver.delete(&v.path).await;
        }
        let old_hash = r.hash.clone();
        if let Err(e) = record::replace_blob(
            ctx.db(),
            id,
            &ingested.hash,
            ingested.size as i64,
            &ingested.content_type,
            width,
            height,
        )
        .await
        {
            tracing::error!(error = %e, "a crop could not be recorded");
            return AdminError::Internal(anyhow::anyhow!(e)).into_response();
        }
        // The old bytes go when this was their last record.
        let still_used = record::find_all_by_hash(ctx.db(), &old_hash)
            .await
            .map(|rs| rs.iter().any(|x| x.disk == r.disk))
            .unwrap_or(true);
        if !still_used && old_hash != ingested.hash {
            let _ = driver.delete(&blob_path(&old_hash)).await;
        }
        ctx.audit(
            &user,
            "media.crop",
            Some("media"),
            Some(&id.to_string()),
            Some(&r.original_name),
            Some(&format!(
                "{{\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"replaced\":true}}",
                form.x, form.y, form.w, form.h
            )),
        )
        .await;
        session.push_flash(FlashLevel::Success, t!("Image cropped."));
        return Redirect::to(&here).into_response();
    }
    let name = cropped_name(&r.original_name, rendered.extension);
    let new_id = match record::create_with(
        ctx.db(),
        NewMedia {
            disk: r.disk.clone(),
            hash: ingested.hash,
            size: ingested.size as i64,
            content_type: ingested.content_type,
            original_name: name.clone(),
            in_library: true,
            created_by: Some(user.user.id),
        },
        NewMediaExtra {
            folder_id: r.folder_id,
            width,
            height,
            meta: None,
        },
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "a crop could not be recorded");
            return AdminError::Internal(anyhow::anyhow!(e)).into_response();
        }
    };
    ctx.audit(
        &user,
        "media.crop",
        Some("media"),
        Some(&new_id.to_string()),
        Some(&name),
        Some(&format!(
            "{{\"from\":{id},\"x\":{},\"y\":{},\"w\":{},\"h\":{}}}",
            form.x, form.y, form.w, form.h
        )),
    )
    .await;
    session.push_flash(FlashLevel::Success, t!("Cropped copy saved."));
    Redirect::to(&ctx.url(&new_id.to_string())).into_response()
}

/// `photo.jpg` cropped becomes `photo-crop.jpg`, in the crop's own format.
fn cropped_name(original: &str, extension: &str) -> String {
    let stem = match original.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => original,
    };
    format!("{stem}-crop.{extension}")
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
    let variants = variant::list_for(ctx.db(), id).await.unwrap_or_default();
    let _ = variant::delete_for(ctx.db(), id).await;
    match record::delete(ctx.db(), id).await {
        Ok(last) => {
            if last {
                if let Some((driver, _)) = app.disks.by_name.get(&r.disk) {
                    for v in &variants {
                        let _ = driver.delete(&v.path).await;
                    }
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
