//! Boards: a dashboard any screen can host.
//!
//! A [`Board`] lays widgets out on a twenty-column grid, draws its page at
//! once and lets each widget fetch itself, and (unless it is fixed) gives the
//! people who may an edit mode: move, resize, add, remove and configure, kept
//! as a layout of their own or one everyone shares.
//!
//! A module registers the kinds of widget it offers with [`KindReg`]; a screen
//! builds a `Board`, puts [`Board::body`] in its page and mounts
//! [`Board::routes`] beside it. The page lists [`BOARD_ASSETS`] in its
//! [`PageHead`](crate::PageHead).

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use laterite_core::{t, Text};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::form::FormField;
use crate::html::Markup;
use crate::routes::RouteCtx;
use crate::{AuthenticatedUser, FlashLevel, SessionHandle, Shell};

/// The grid's columns.
pub const COLUMNS: u8 = 20;
/// The narrowest a widget may be made, whatever its kind allows.
pub const MIN_WIDTH: u8 = 3;
/// The assets a page hosting a board lists in its head.
pub const BOARD_ASSETS: &[&str] = &["board.css", "board.js"];

/// What a widget draws from: the request, its viewer, and the board's query
/// string (a period, say).
pub struct BoardCx<'a> {
    /// The routes' context: the database, the cache, the registries.
    pub ctx: &'a RouteCtx,
    /// The viewer's shell: translation, permissions, formatting.
    pub shell: &'a Shell,
    /// The viewer.
    pub user: &'a AuthenticatedUser,
    /// The page's query string, passed on to each widget.
    pub query: &'a HashMap<String, String>,
    /// The columns the widget spans.
    pub width: u8,
}

/// A kind of widget a board can hold.
#[laterite_core::strata::async_trait]
pub trait Kind: Send + Sync {
    /// The kind's dotted code, kept in every layout that places it.
    fn code(&self) -> &str;
    /// Its name in the Add menu.
    fn label(&self) -> Text;
    /// The heading it is listed under in the Add menu.
    fn group(&self) -> Text {
        t!("Widgets")
    }
    /// What it shows, under its name in the Add menu.
    fn description(&self) -> Option<Text> {
        None
    }
    /// The permission needed to see or add it.
    fn permission(&self) -> Option<&str> {
        None
    }
    /// The columns a newly added one spans.
    fn default_width(&self) -> u8 {
        7
    }
    /// The fewest columns it may span.
    fn min_width(&self) -> u8 {
        MIN_WIDTH
    }
    /// The most columns it may span.
    fn max_width(&self) -> u8 {
        COLUMNS
    }
    /// Whether its width may be changed.
    fn resizable(&self) -> bool {
        true
    }
    /// Whether a board holds at most one.
    fn once(&self) -> bool {
        false
    }
    /// Drawn without the card around it: a section heading.
    fn bare(&self) -> bool {
        false
    }
    /// The settings one placement carries, as form fields. A kind with none
    /// has no Configure.
    fn properties(&self) -> Vec<FormField> {
        Vec::new()
    }
    /// The settings fields for a widget whose settings are `current`, where
    /// they depend on one another: the metrics a chosen source offers, say.
    /// [`Kind::properties`] unless said.
    fn properties_for(&self, _cx: &BoardCx<'_>, _current: &Value) -> Vec<FormField> {
        self.properties()
    }
    /// The settings whose change draws the Configure form again through
    /// [`Kind::properties_for`].
    fn reload_on(&self) -> Vec<&'static str> {
        Vec::new()
    }
    /// Whether settings a person entered can be kept; an error is shown to
    /// them and nothing is saved.
    fn check(&self, _properties: &Value) -> Result<(), String> {
        Ok(())
    }
    /// The heading above the widget, from its settings.
    fn title(&self, _shell: &Shell, _properties: &Value) -> Option<String> {
        None
    }
    /// Seconds between the widget fetching itself again; `None` is never.
    fn refresh(&self, _properties: &Value) -> Option<u32> {
        None
    }
    /// The widget's content. An error is shown in the widget's place.
    async fn render(&self, cx: &BoardCx<'_>, properties: &Value) -> Result<Markup, String>;
}

/// A registered kind of widget.
///
/// ```ignore
/// registry.add(KindReg::new(Arc::new(SalesToday)));
/// ```
#[derive(Clone)]
pub struct KindReg {
    /// The kind.
    pub kind: Arc<dyn Kind>,
}

impl KindReg {
    /// Registers `kind` for every board that offers it.
    pub fn new(kind: Arc<dyn Kind>) -> Self {
        Self { kind }
    }
}

fn yes() -> bool {
    true
}

/// A widget on a board: a kind, how wide, and its settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// What tells it from the board's other widgets; kept across edits.
    pub id: String,
    /// The kind's code.
    pub kind: String,
    /// The columns it spans.
    pub width: u8,
    /// Its settings, by property name.
    #[serde(default)]
    pub properties: Value,
    /// Stays where the board's author put it: not moved, resized or removed.
    #[serde(default)]
    pub locked: bool,
    /// May be taken off the board.
    #[serde(default = "yes")]
    pub removable: bool,
    /// Its settings may be changed.
    #[serde(default = "yes")]
    pub configurable: bool,
}

impl Placement {
    /// A widget of `kind`, `width` columns wide, with no settings.
    pub fn new(id: impl Into<String>, kind: impl Into<String>, width: u8) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            width,
            properties: Value::Object(Default::default()),
            locked: false,
            removable: true,
            configurable: true,
        }
    }

    /// Its settings.
    pub fn properties(mut self, properties: Value) -> Self {
        self.properties = properties;
        self
    }

    /// Pins it: not moved, resized or removed.
    pub fn locked(mut self) -> Self {
        self.locked = true;
        self
    }

    /// Keeps it on the board: it has no Remove.
    pub fn keep(mut self) -> Self {
        self.removable = false;
        self
    }

    /// Fixes its settings: it has no Configure.
    pub fn fixed_settings(mut self) -> Self {
        self.configurable = false;
        self
    }
}

/// A board's widgets in rows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// The rows, each a list of widgets left to right.
    #[serde(default)]
    pub rows: Vec<Vec<Placement>>,
}

impl Layout {
    /// A layout of these rows.
    pub fn rows(rows: Vec<Vec<Placement>>) -> Self {
        Self { rows }
    }

    fn find(&self, id: &str) -> Option<&Placement> {
        self.rows.iter().flatten().find(|p| p.id == id)
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut Placement> {
        self.rows.iter_mut().flatten().find(|p| p.id == id)
    }
}

/// Who may arrange a board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Editing {
    /// Nobody: the board is as its author made it.
    Off,
    /// Each person arranges their own.
    Personal,
    /// One layout for everyone, changed by whoever may.
    Shared,
    /// Each person their own, over a shared one.
    #[default]
    Both,
}

/// What edit mode offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allow {
    /// Dragging a widget elsewhere.
    pub moving: bool,
    /// Changing a widget's width.
    pub resizing: bool,
    /// Adding a widget.
    pub adding: bool,
    /// Taking a widget off.
    pub removing: bool,
    /// Changing a widget's settings.
    pub configuring: bool,
}

impl Default for Allow {
    fn default() -> Self {
        Self {
            moving: true,
            resizing: true,
            adding: true,
            removing: true,
            configuring: true,
        }
    }
}

/// What the layout an editor sends says about one widget.
#[derive(Deserialize)]
struct Sent {
    id: String,
    width: u8,
}

/// A dashboard a screen hosts.
#[derive(Clone)]
pub struct Board {
    key: String,
    editing: Editing,
    view_permission: Option<String>,
    edit_permission: Option<String>,
    share_permission: Option<String>,
    allow: Allow,
    offered: Option<Vec<String>>,
    packed: bool,
    carry: Vec<String>,
    default: Layout,
    own: Vec<Arc<dyn Kind>>,
}

impl Board {
    /// A board named `key` (`acme.sales`), which its layouts are kept under,
    /// starting from `default`.
    pub fn new(key: impl Into<String>, default: Layout) -> Self {
        Self {
            key: key.into(),
            editing: Editing::default(),
            view_permission: None,
            edit_permission: None,
            share_permission: None,
            allow: Allow::default(),
            offered: None,
            packed: true,
            carry: Vec::new(),
            default,
            own: Vec::new(),
        }
    }

    /// Who may arrange it. [`Editing::Both`] unless said.
    pub fn editing(mut self, editing: Editing) -> Self {
        self.editing = editing;
        self
    }

    /// The permission needed to open the board; its routes refuse a request
    /// without it. Without one, whoever reaches the hosting screen may.
    pub fn view_permission(mut self, permission: impl Into<String>) -> Self {
        self.view_permission = Some(permission.into());
        self
    }

    /// The permission needed to arrange one's own. Without one, whoever sees
    /// the board may.
    pub fn edit_permission(mut self, permission: impl Into<String>) -> Self {
        self.edit_permission = Some(permission.into());
        self
    }

    /// The permission needed to change the layout everyone shares. Without
    /// one, nobody may.
    pub fn share_permission(mut self, permission: impl Into<String>) -> Self {
        self.share_permission = Some(permission.into());
        self
    }

    /// What edit mode offers.
    pub fn allow(mut self, allow: Allow) -> Self {
        self.allow = allow;
        self
    }

    /// The kinds that may be added, by code; a code ending in `.*` offers
    /// every kind under that prefix. Every registered kind unless said.
    pub fn offer<S: Into<String>>(mut self, kinds: impl IntoIterator<Item = S>) -> Self {
        self.offered = Some(kinds.into_iter().map(Into::into).collect());
        self
    }

    /// Keeps each row on its own line, in place of packing a widget under
    /// the one above it in its columns.
    pub fn rows(mut self) -> Self {
        self.packed = false;
        self
    }

    /// Query parameters the board's own requests carry along, such as a
    /// period's `range`, `from` and `to`.
    pub fn carry<S: Into<String>>(mut self, names: impl IntoIterator<Item = S>) -> Self {
        self.carry = names.into_iter().map(Into::into).collect();
        self
    }

    /// A kind this board alone offers, beside the registered ones.
    pub fn with_kind(mut self, kind: Arc<dyn Kind>) -> Self {
        self.own.push(kind);
        self
    }

    /// The board's key.
    pub fn key(&self) -> &str {
        &self.key
    }

    fn preference(&self) -> String {
        format!("board.{}", self.key)
    }

    fn parameter(&self) -> String {
        format!("laterite.admin::boards.{}", self.key)
    }

    fn may_view(&self, shell: &Shell) -> bool {
        self.view_permission
            .as_deref()
            .is_none_or(|p| shell.allows(p))
    }

    fn may_edit_own(&self, shell: &Shell) -> bool {
        self.may_view(shell)
            && matches!(self.editing, Editing::Personal | Editing::Both)
            && self
                .edit_permission
                .as_deref()
                .is_none_or(|p| shell.allows(p))
    }

    fn may_share(&self, shell: &Shell) -> bool {
        self.may_view(shell)
            && matches!(self.editing, Editing::Shared | Editing::Both)
            && self
                .share_permission
                .as_deref()
                .is_some_and(|p| shell.allows(p))
    }

    fn offers(&self, code: &str) -> bool {
        self.offered.as_ref().is_none_or(|list| {
            list.iter().any(|o| match o.strip_suffix(".*") {
                Some(prefix) => code.starts_with(prefix) && code[prefix.len()..].starts_with('.'),
                None => o == code,
            })
        })
    }

    /// The layout `user` sees, and whether it is their own.
    async fn layout(&self, ctx: &RouteCtx, user: &AuthenticatedUser) -> (Layout, bool) {
        if matches!(self.editing, Editing::Personal | Editing::Both) {
            let own =
                laterite_auth::store::user_preference(ctx.db(), user.user.id, &self.preference())
                    .await
                    .ok()
                    .flatten()
                    .and_then(|json| serde_json::from_str::<Layout>(&json).ok());
            if let Some(own) = own {
                return (own, true);
            }
        }
        if matches!(self.editing, Editing::Shared | Editing::Both) {
            if let Ok(Some(shared)) = ctx.parameters().get::<Layout>(&self.parameter()).await {
                return (shared, false);
            }
        }
        (self.default.clone(), false)
    }

    /// The layout an editor sent, made safe: each widget one the current
    /// layout or the default has, a width within its kind's bounds, a locked
    /// widget where the default put it, and an overfull row split.
    fn checked(&self, sent: &str, current: &Layout, kinds: &Kinds<'_>) -> Result<Layout, String> {
        let sent: Vec<Vec<Sent>> = serde_json::from_str(sent).map_err(|e| e.to_string())?;
        let mut seen: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Placement>> = Vec::new();
        for row in sent {
            let mut line: Vec<Placement> = Vec::new();
            let mut used: u16 = 0;
            for item in row {
                let Some(known) = current
                    .find(&item.id)
                    .or_else(|| self.default.find(&item.id))
                else {
                    continue;
                };
                if seen.contains(&item.id) {
                    continue;
                }
                let mut placed = known.clone();
                if !placed.locked {
                    placed.width = match kinds.get(&placed.kind) {
                        Some(kind) if !kind.resizable() || !self.allow.resizing => placed.width,
                        Some(kind) => item
                            .width
                            .clamp(kind.min_width().max(1), kind.max_width().min(COLUMNS)),
                        None => placed.width,
                    };
                }
                placed.width = placed.width.clamp(1, COLUMNS);
                if used + u16::from(placed.width) > u16::from(COLUMNS) && !line.is_empty() {
                    rows.push(std::mem::take(&mut line));
                    used = 0;
                }
                used += u16::from(placed.width);
                seen.push(placed.id.clone());
                line.push(placed);
            }
            if !line.is_empty() {
                rows.push(line);
            }
        }
        // What may not be removed comes back, each in a row of its own.
        for kept in current.rows.iter().flatten() {
            let stays = kept.locked || !kept.removable || !self.allow.removing;
            if stays && !seen.contains(&kept.id) {
                seen.push(kept.id.clone());
                rows.push(vec![kept.clone()]);
            }
        }
        Ok(Layout { rows })
    }

    /// Whether the viewer may arrange the board at all.
    pub fn editable(&self, shell: &Shell) -> bool {
        self.may_edit_own(shell) || self.may_share(shell)
    }

    /// The form assets the Configure dialogs of this board's kinds need,
    /// beyond [`BOARD_ASSETS`].
    pub fn assets(&self, ctx: &RouteCtx) -> Vec<&'static str> {
        let mut assets: Vec<&'static str> = BOARD_ASSETS.to_vec();
        let kinds = Kinds::of(ctx, &self.own);
        for kind in kinds.all() {
            let fields = kind.properties();
            if fields.is_empty() || !self.offers(kind.code()) {
                continue;
            }
            if let Ok(form) = ctx.value_form(&fields, "laterite.admin.board") {
                for asset in form.assets() {
                    if !assets.contains(&asset) {
                        assets.push(asset);
                    }
                }
            }
        }
        assets
    }

    fn carried(&self, query: &HashMap<String, String>) -> String {
        let pairs: Vec<String> = self
            .carry
            .iter()
            .filter_map(|name| {
                query.get(name).map(|value| {
                    format!(
                        "{}={}",
                        name,
                        value
                            .bytes()
                            .map(|b| match b {
                                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                                    (b as char).to_string()
                                }
                                _ => format!("%{b:02X}"),
                            })
                            .collect::<String>()
                    )
                })
            })
            .collect();
        if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", pairs.join("&"))
        }
    }

    /// The board, for the page: every widget's frame at once, each widget
    /// then fetching itself from the routes mounted at `base`.
    pub async fn body(&self, cx: &BoardCx<'_>, base: &str) -> Markup {
        let kinds = Kinds::of(cx.ctx, &self.own);
        let (layout, personal) = self.layout(cx.ctx, cx.user).await;
        let shell = cx.shell;
        let mut widgets = Vec::new();
        for (index, row) in layout.rows.iter().enumerate() {
            let mut start: u16 = 1;
            for placed in row {
                let width = placed.width.clamp(1, COLUMNS);
                let Some(kind) = kinds.get(&placed.kind) else {
                    continue;
                };
                if kind.permission().is_some_and(|p| !shell.allows(p)) {
                    continue;
                }
                let free = !placed.locked;
                widgets.push(WidgetView {
                    id: placed.id.clone(),
                    row: index + 1,
                    width,
                    start,
                    min: kind.min_width().max(1),
                    max: kind.max_width().min(COLUMNS),
                    bare: kind.bare(),
                    movable: free && self.allow.moving,
                    resizable: free && self.allow.resizing && kind.resizable(),
                    removable: free && self.allow.removing && placed.removable,
                    configurable: free
                        && self.allow.configuring
                        && placed.configurable
                        && !kind.properties().is_empty(),
                    title: kind
                        .title(shell, &placed.properties)
                        .filter(|t| !t.is_empty()),
                    refresh: kind
                        .refresh(&placed.properties)
                        .map(|s| format!(", every {}s", s.max(5)))
                        .unwrap_or_default(),
                });
                start += u16::from(width);
            }
        }
        let bar = self
            .editable(shell)
            .then(|| self.bar(shell, personal, &kinds, &layout));
        let view = BoardTemplate {
            shell,
            base,
            carried: self.carried(cx.query),
            packed: self.packed,
            editing: cx.query.get("edit").is_some_and(|v| v == "1") && self.editable(shell),
            bar,
            widgets,
        };
        Markup::from_template(&view).unwrap_or_else(|e| {
            tracing::error!(board = %self.key, error = %e, "a board could not be drawn");
            Markup::empty()
        })
    }

    /// What the edit bar offers this viewer.
    fn bar(&self, shell: &Shell, personal: bool, kinds: &Kinds<'_>, layout: &Layout) -> BarView {
        let mut groups: Vec<AddGroup> = Vec::new();
        if self.allow.adding {
            for kind in kinds.all() {
                if !self.offers(kind.code()) || kind.permission().is_some_and(|p| !shell.allows(p))
                {
                    continue;
                }
                // One that sits once and is already here is not offered again.
                if kind.once() && layout.rows.iter().flatten().any(|p| p.kind == kind.code()) {
                    continue;
                }
                let label = shell.tt(&kind.group());
                let item = AddItem {
                    code: kind.code().to_string(),
                    label: shell.tt(&kind.label()),
                    description: kind.description().map(|d| shell.tt(&d)).unwrap_or_default(),
                };
                match groups.iter_mut().find(|g| g.label == label) {
                    Some(group) => group.items.push(item),
                    None => groups.push(AddGroup {
                        label,
                        items: vec![item],
                    }),
                }
            }
        }
        BarView {
            own: self.may_edit_own(shell),
            share: self.may_share(shell),
            personal,
            groups,
        }
    }

    /// The board's routes, mounted where [`Board::body`] was given as `base`:
    /// a widget's content, the layout, and a widget's settings.
    pub fn routes(self: Arc<Self>, ctx: &RouteCtx) -> Router {
        let (one, save, form, set) = (
            (self.clone(), ctx.clone()),
            (self.clone(), ctx.clone()),
            (self.clone(), ctx.clone()),
            (self.clone(), ctx.clone()),
        );
        Router::new()
            .route(
                "/widgets/{id}",
                get(
                    move |Path(id): Path<String>,
                          Query(query): Query<HashMap<String, String>>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>| {
                        let (board, ctx) = one.clone();
                        async move { board.widget(&ctx, &shell, &user, &query, &id).await }
                    },
                ),
            )
            .route(
                "/layout",
                post(
                    move |Query(query): Query<HashMap<String, String>>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>,
                          Extension(session): Extension<SessionHandle>,
                          headers: axum::http::HeaderMap,
                          Form(form): Form<HashMap<String, String>>| {
                        let (board, ctx) = save.clone();
                        async move {
                            board
                                .arrange(&ctx, &shell, &user, &session, &query, &headers, &form)
                                .await
                        }
                    },
                ),
            )
            .route(
                "/configure/{id}",
                get(
                    move |Path(id): Path<String>,
                          Query(query): Query<HashMap<String, String>>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>| {
                        let (board, ctx) = form.clone();
                        async move { board.settings(&ctx, &shell, &user, &query, &id).await }
                    },
                )
                .post(
                    move |Path(id): Path<String>,
                          Query(query): Query<HashMap<String, String>>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>,
                          Extension(session): Extension<SessionHandle>,
                          headers: axum::http::HeaderMap,
                          Form(form): Form<HashMap<String, String>>| {
                        let (board, ctx) = set.clone();
                        async move {
                            board
                                .configure(
                                    &ctx, &shell, &user, &session, &query, &headers, &id, &form,
                                )
                                .await
                        }
                    },
                ),
            )
    }

    async fn widget(
        &self,
        ctx: &RouteCtx,
        shell: &Shell,
        user: &AuthenticatedUser,
        query: &HashMap<String, String>,
        id: &str,
    ) -> Response {
        if !self.may_view(shell) {
            return StatusCode::NOT_FOUND.into_response();
        }
        let kinds = Kinds::of(ctx, &self.own);
        let (layout, _) = self.layout(ctx, user).await;
        let Some(placed) = layout.find(id) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(kind) = kinds.get(&placed.kind) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if kind.permission().is_some_and(|p| !shell.allows(p)) {
            return StatusCode::NOT_FOUND.into_response();
        }
        let cx = BoardCx {
            ctx,
            shell,
            user,
            query,
            width: placed.width,
        };
        let body = match kind.render(&cx, &placed.properties).await {
            Ok(markup) => markup.into_string(),
            Err(reason) => Markup::from_template(&FailedTemplate { reason: &reason })
                .map(Markup::into_string)
                .unwrap_or_default(),
        };
        ([(header::CACHE_CONTROL, "private, no-store")], Html(body)).into_response()
    }

    /// Where the board's page is: what the routes send the browser back to.
    fn back(
        headers: &axum::http::HeaderMap,
        query: &HashMap<String, String>,
        edit: bool,
    ) -> Response {
        // The page that posted, without its query; the carried parameters and
        // the edit flag are put back.
        let page = headers
            .get(header::REFERER)
            .and_then(|v| v.to_str().ok())
            .and_then(|url| url.split_once("://").map(|(_, rest)| rest))
            .and_then(|rest| rest.find('/').map(|at| &rest[at..]))
            .map(|path| path.split(['?', '#']).next().unwrap_or(path).to_string())
            .filter(|path| path.starts_with('/') && !path.starts_with("//"))
            .unwrap_or_else(|| "/".to_string());
        let mut pairs: Vec<String> = query
            .iter()
            .filter(|(name, _)| name.as_str() != "edit")
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        pairs.sort();
        if edit {
            pairs.push("edit=1".into());
        }
        let to = if pairs.is_empty() {
            page
        } else {
            format!("{page}?{}", pairs.join("&"))
        };
        (StatusCode::SEE_OTHER, [(header::LOCATION, to)]).into_response()
    }

    /// Where an edit in progress is kept: the editor's own layout when they
    /// have one to keep, else the shared one.
    fn working_scope(&self, shell: &Shell) -> Option<&'static str> {
        if self.may_edit_own(shell) {
            Some("mine")
        } else if self.may_share(shell) {
            Some("shared")
        } else {
            None
        }
    }

    async fn store(
        &self,
        ctx: &RouteCtx,
        user: &AuthenticatedUser,
        scope: &str,
        layout: &Layout,
    ) -> Result<(), String> {
        let json = serde_json::to_string(layout).map_err(|e| e.to_string())?;
        match scope {
            "shared" => {
                ctx.parameters()
                    .set(&self.parameter(), layout)
                    .await
                    .map_err(|e| e.to_string())?;
                // Their own goes, so they see what everyone now sees.
                let _ = laterite_auth::store::clear_user_preference(
                    ctx.db(),
                    user.user.id,
                    &self.preference(),
                )
                .await;
                Ok(())
            }
            _ => laterite_auth::store::set_user_preference(
                ctx.db(),
                user.user.id,
                &self.preference(),
                &json,
            )
            .await
            .map_err(|e| e.to_string()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn arrange(
        &self,
        ctx: &RouteCtx,
        shell: &Shell,
        user: &AuthenticatedUser,
        session: &SessionHandle,
        query: &HashMap<String, String>,
        headers: &axum::http::HeaderMap,
        form: &HashMap<String, String>,
    ) -> Response {
        let scope = form.get("scope").map(String::as_str).unwrap_or_default();
        let adding = form
            .get("add")
            .map(String::as_str)
            .filter(|a| !a.is_empty());
        let allowed = match (scope, adding) {
            ("shared", None) => self.may_share(shell),
            ("mine" | "reset", None) => self.may_edit_own(shell),
            (_, Some(_)) => self.allow.adding && self.editable(shell),
            _ => false,
        };
        if !allowed {
            return StatusCode::FORBIDDEN.into_response();
        }
        if scope == "reset" && adding.is_none() {
            let _ = laterite_auth::store::clear_user_preference(
                ctx.db(),
                user.user.id,
                &self.preference(),
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Your own layout was removed."));
            return Self::back(headers, query, false);
        }
        let kinds = Kinds::of(ctx, &self.own);
        let (current, _) = self.layout(ctx, user).await;
        let sent = form.get("layout").map(String::as_str).unwrap_or_default();
        let mut layout = if sent.is_empty() {
            current.clone()
        } else {
            match self.checked(sent, &current, &kinds) {
                Ok(layout) => layout,
                Err(e) => {
                    session.push_flash(
                        FlashLevel::Error,
                        Text::dynamic(format!("The layout could not be read: {e}")),
                    );
                    return Self::back(headers, query, true);
                }
            }
        };
        let mut note = None;
        let scope = match adding {
            Some(code) => {
                let Some(kind) = kinds
                    .get(code)
                    .filter(|k| self.offers(k.code()))
                    .filter(|k| k.permission().is_none_or(|p| shell.allows(p)))
                else {
                    return StatusCode::NOT_FOUND.into_response();
                };
                if kind.once() && layout.rows.iter().flatten().any(|p| p.kind == code) {
                    session.push_flash(
                        FlashLevel::Info,
                        t!("This dashboard already has that widget."),
                    );
                    return Self::back(headers, query, true);
                }
                let mut n = 1;
                let id = loop {
                    let id = format!("w{n}");
                    if layout.find(&id).is_none() && self.default.find(&id).is_none() {
                        break id;
                    }
                    n += 1;
                };
                let width = kind
                    .default_width()
                    .clamp(kind.min_width().max(1), kind.max_width().min(COLUMNS));
                layout
                    .rows
                    .push(vec![Placement::new(id.clone(), code, width)]);
                if !kind.properties().is_empty() {
                    note = Some(id);
                }
                self.working_scope(shell).unwrap_or("mine")
            }
            None => scope,
        };
        match self.store(ctx, user, scope, &layout).await {
            Ok(()) => {
                ctx.audit(
                    user,
                    "laterite.admin.board.arrange",
                    Some("boards"),
                    Some(&self.key),
                    None,
                    Some(if adding.is_some() { "add" } else { scope }),
                )
                .await;
                if adding.is_none() {
                    session.push_flash(
                        FlashLevel::Success,
                        if scope == "shared" {
                            t!("The layout was saved for everyone.")
                        } else {
                            t!("Your layout was saved.")
                        },
                    );
                }
            }
            Err(e) => session.push_flash(
                FlashLevel::Error,
                Text::dynamic(format!("The layout could not be saved: {e}")),
            ),
        }
        // A widget just added with settings opens them when the page returns.
        let mut query = query.clone();
        if let Some(id) = note {
            query.insert("configure".into(), id);
        }
        Self::back(headers, &query, adding.is_some())
    }

    /// A widget's settings form, for the Configure dialog.
    async fn settings(
        &self,
        ctx: &RouteCtx,
        shell: &Shell,
        user: &AuthenticatedUser,
        query: &HashMap<String, String>,
        id: &str,
    ) -> Response {
        if !self.allow.configuring || !self.editable(shell) {
            return StatusCode::FORBIDDEN.into_response();
        }
        let kinds = Kinds::of(ctx, &self.own);
        let (layout, _) = self.layout(ctx, user).await;
        let Some(placed) = layout.find(id).filter(|p| p.configurable && !p.locked) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(kind) = kinds.get(&placed.kind) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        // The form drawn again after a setting others depend on changed
        // carries what is in it so far, each name under `p.`.
        let mut current = placed.properties.clone();
        if !current.is_object() {
            current = Value::Object(Default::default());
        }
        if let Value::Object(map) = &mut current {
            for (name, value) in query {
                if let Some(name) = name.strip_prefix("p.") {
                    map.insert(name.to_string(), Value::String(value.clone()));
                }
            }
        }
        let cx = BoardCx {
            ctx,
            shell,
            user,
            query,
            width: placed.width,
        };
        let form = match ctx.value_form(&kind.properties_for(&cx, &current), "laterite.admin.board")
        {
            Ok(form) => form,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, shell.tt(&e)).into_response(),
        };
        let view = SettingsTemplate {
            shell,
            carried: self.carried(query),
            title: shell.tt(&kind.label()),
            reload: kind.reload_on().join(","),
            fields: form.render(&current, shell).into_string(),
        };
        let body = match Markup::from_template(&view) {
            Ok(markup) => markup.into_string(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        ([(header::CACHE_CONTROL, "private, no-store")], Html(body)).into_response()
    }

    #[allow(clippy::too_many_arguments)]
    async fn configure(
        &self,
        ctx: &RouteCtx,
        shell: &Shell,
        user: &AuthenticatedUser,
        session: &SessionHandle,
        query: &HashMap<String, String>,
        headers: &axum::http::HeaderMap,
        id: &str,
        data: &HashMap<String, String>,
    ) -> Response {
        let Some(scope) = self.working_scope(shell).filter(|_| self.allow.configuring) else {
            return StatusCode::FORBIDDEN.into_response();
        };
        let kinds = Kinds::of(ctx, &self.own);
        let (current, _) = self.layout(ctx, user).await;
        // The arrangement in progress is kept along with the settings.
        let sent = data.get("layout").map(String::as_str).unwrap_or_default();
        let mut layout = if sent.is_empty() {
            current.clone()
        } else {
            self.checked(sent, &current, &kinds)
                .unwrap_or_else(|_| current.clone())
        };
        let Some(placed) = layout.find_mut(id).filter(|p| p.configurable && !p.locked) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(kind) = kinds.get(&placed.kind) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let sent_values = Value::Object(
            data.iter()
                .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                .collect(),
        );
        let cx = BoardCx {
            ctx,
            shell,
            user,
            query,
            width: placed.width,
        };
        let collected = ctx
            .value_form(
                &kind.properties_for(&cx, &sent_values),
                "laterite.admin.board",
            )
            .and_then(|form| form.collect(data));
        let collected = collected.and_then(|properties| {
            kind.check(&properties)
                .map(|()| properties)
                .map_err(Text::dynamic)
        });
        match collected {
            Ok(properties) => {
                placed.properties = properties;
                match self.store(ctx, user, scope, &layout).await {
                    Ok(()) => {
                        ctx.audit(
                            user,
                            "laterite.admin.board.configure",
                            Some("boards"),
                            Some(&self.key),
                            Some(id),
                            None,
                        )
                        .await;
                    }
                    Err(e) => session.push_flash(
                        FlashLevel::Error,
                        Text::dynamic(format!("The settings could not be saved: {e}")),
                    ),
                }
            }
            Err(e) => session.push_flash(FlashLevel::Error, e),
        }
        Self::back(headers, query, true)
    }
}

/// The kinds a board draws from: its own, then the registered ones.
struct Kinds<'a> {
    own: &'a [Arc<dyn Kind>],
    regs: Vec<&'a KindReg>,
}

impl<'a> Kinds<'a> {
    fn of(ctx: &'a RouteCtx, own: &'a [Arc<dyn Kind>]) -> Self {
        Self {
            own,
            regs: ctx.contributions::<KindReg>(),
        }
    }

    fn get(&self, code: &str) -> Option<&dyn Kind> {
        self.all().find(|k| k.code() == code)
    }

    fn all(&self) -> impl Iterator<Item = &dyn Kind> + '_ {
        self.own
            .iter()
            .map(|k| k.as_ref())
            .chain(self.regs.iter().map(|r| r.kind.as_ref()))
    }
}

#[derive(Template)]
#[template(path = "board/board.html")]
struct BoardTemplate<'a> {
    shell: &'a Shell,
    base: &'a str,
    carried: String,
    packed: bool,
    editing: bool,
    bar: Option<BarView>,
    widgets: Vec<WidgetView>,
}

struct BarView {
    own: bool,
    share: bool,
    personal: bool,
    groups: Vec<AddGroup>,
}

struct AddGroup {
    label: String,
    items: Vec<AddItem>,
}

struct AddItem {
    code: String,
    label: String,
    description: String,
}

struct WidgetView {
    id: String,
    row: usize,
    width: u8,
    start: u16,
    min: u8,
    max: u8,
    bare: bool,
    movable: bool,
    resizable: bool,
    removable: bool,
    configurable: bool,
    title: Option<String>,
    refresh: String,
}

#[derive(Template)]
#[template(path = "board/settings.html")]
struct SettingsTemplate<'a> {
    shell: &'a Shell,
    carried: String,
    title: String,
    reload: String,
    fields: String,
}

#[derive(Template)]
#[template(path = "board/failed.html")]
struct FailedTemplate<'a> {
    reason: &'a str,
}
