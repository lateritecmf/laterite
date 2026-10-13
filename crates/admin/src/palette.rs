//! The command palette: one box that finds a page or a record by name and
//! goes there. It knows the menu the operator may see, the settings they may
//! open, and the records of every resource that offered its picker source.
//!
//! With scripting it is an overlay on every page, opened from the search
//! button or with Ctrl+K (Cmd+K); without, the same button is a page with a
//! search form, served by the same handler.

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use laterite_auth::AuthenticatedUser;
use laterite_core::Text;
use serde::{Deserialize, Serialize};

use crate::{nav, render, AdminState, Shell};

/// A resource whose records the palette finds: its picker source, where a
/// record's edit page lives, and what the operator must hold to see them.
#[derive(Clone, Debug)]
pub(crate) struct RecordSource {
    pub label: Text,
    pub source: String,
    /// The resource's path under the admin mount.
    pub base_path: String,
    pub permission: Option<String>,
}

/// One thing the palette found.
#[derive(Debug, Serialize)]
pub(crate) struct Hit {
    pub label: String,
    /// Where it sits: the section, the settings category, the resource.
    pub hint: String,
    pub href: String,
    /// `page`, `setting` or `record`, for the overlay to group by.
    pub kind: &'static str,
}

#[derive(Deserialize)]
pub(crate) struct PaletteQuery {
    #[serde(default)]
    q: String,
}

/// Records per source in one search, and the longest query accepted.
const RECORD_LIMIT: u32 = 8;
const MAX_QUERY: usize = 120;

fn matches(haystack: &str, needle: &str) -> bool {
    needle.is_empty() || haystack.to_lowercase().contains(needle)
}

/// Finds what `q` names for this operator: menu entries and settings by a
/// case-insensitive substring, then records from the sources they may see.
/// A blank query lists the pages alone.
pub(crate) async fn find(
    state: &AdminState,
    user: &AuthenticatedUser,
    shell: &Shell,
    q: &str,
) -> Vec<Hit> {
    let q = q.trim();
    let q = &q.chars().take(MAX_QUERY).collect::<String>().to_lowercase();
    let mut hits = Vec::new();
    let allows = |code: &str| user.permissions.allows(code);
    for node in state.nav.visible(&allows) {
        let section = shell.tt(&node.item.label);
        if let Some(path) = nav::tab_path(&node) {
            if matches(&section, q) {
                hits.push(Hit {
                    label: section.clone(),
                    hint: String::new(),
                    href: path.to_string(),
                    kind: "page",
                });
            }
        }
        for child in &node.children {
            let label = shell.tt(&child.item.label);
            if let Some(path) = &child.item.path {
                if matches(&label, q) {
                    hits.push(Hit {
                        label,
                        hint: section.clone(),
                        href: path.clone(),
                        kind: "page",
                    });
                }
            }
        }
    }
    for item in state.settings.iter() {
        if !item.permission.as_deref().is_none_or(allows) {
            continue;
        }
        let label = shell.tt(&item.label);
        if matches(&label, q) {
            hits.push(Hit {
                label,
                hint: shell.tt(&item.category),
                href: item.path(&state.admin_path),
                kind: "setting",
            });
        }
    }
    if q.is_empty() {
        return hits;
    }
    for source in state.palette.iter() {
        if !source.permission.as_deref().is_none_or(allows) {
            continue;
        }
        let Some(reg) = state.pickers.get(&source.source) else {
            continue;
        };
        let found = match reg.source.search(&state.db, q, RECORD_LIMIT).await {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(source = %source.source, error = %e, "palette search failed");
                continue;
            }
        };
        let resource = shell.tt(&source.label);
        for node in found {
            hits.push(Hit {
                label: node.label,
                hint: node
                    .hint
                    .map(|h| format!("{resource} · {h}"))
                    .unwrap_or_else(|| resource.clone()),
                href: format!("{}/{}/edit", source.base_path, node.id),
                kind: "record",
            });
        }
    }
    hits
}

#[derive(Serialize)]
struct Found {
    items: Vec<Hit>,
}

/// The palette's handler: JSON for the overlay, a page with a search form
/// for a request without scripting.
pub(crate) async fn palette(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<PaletteQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let items = find(&state, &user, &shell, &query.q).await;
    if crate::form::is_htmx(&headers) || headers.get("x-requested-with").is_some() {
        return (
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            axum::Json(Found { items }),
        )
            .into_response();
    }
    render(PaletteTemplate {
        shell,
        q: query.q.trim().chars().take(MAX_QUERY).collect(),
        items,
    })
}

#[derive(askama::Template)]
#[template(path = "palette.html")]
struct PaletteTemplate {
    shell: Shell,
    q: String,
    items: Vec<Hit>,
}
