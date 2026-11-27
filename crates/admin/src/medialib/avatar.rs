//! An account's picture: uploaded by its owner or by an administrator, kept as
//! a media record the library does not list, and shown in the top bar.
//!
//! The picture's address is kept among the account's preferences, which every
//! request already reads, so drawing it costs no query.

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;
use laterite_auth::AuthenticatedUser;
use laterite_core::t;
use laterite_media::{ingest, link, record, stream_of, NewMedia, NewMediaExtra, Owner};

use super::{admit, forbidden, variant_url, App, DISK};
use crate::session::{FlashLevel, SessionHandle};
use crate::upload::VerifiedUpload;

/// The preference holding the address of an account's picture.
pub(crate) const PREFERENCE: &str = "backend.avatar";

/// The entity a picture's media link names as its owner.
const OWNER: &str = "backend_users";

/// The largest picture accepted.
const LIMIT: u64 = 5 * 1024 * 1024;

/// Whether `editor` may change the picture of account `target`: their own, or
/// another's with the permission to manage users, a superuser's by a
/// superuser alone.
async fn may_change(app: &super::AppState, editor: &AuthenticatedUser, target: i64) -> bool {
    if editor.user.id == target {
        return true;
    }
    if !editor.permissions.allows(crate::users::MANAGE_USERS) {
        return false;
    }
    match laterite_auth::store::find_user_by_id(app.ctx.db(), target).await {
        Ok(Some(user)) => !user.is_superuser || editor.user.is_superuser,
        _ => false,
    }
}

/// Where a change returns to: Preferences for your own, the account's page
/// for another's.
fn back(app: &super::AppState, editor: &AuthenticatedUser, target: i64) -> Response {
    let admin = app.ctx.admin_path().trim_end_matches('/');
    let to = if editor.user.id == target {
        format!("{admin}/preferences#picture")
    } else {
        format!("{admin}/users/{target}/edit")
    };
    Redirect::to(&to).into_response()
}

/// Sets your own picture.
pub(super) async fn upload_own(
    state: App,
    session: Extension<SessionHandle>,
    Extension(editor): Extension<AuthenticatedUser>,
    upload: VerifiedUpload,
) -> Response {
    let target = editor.user.id;
    store(state, session, editor, target, upload).await
}

/// Sets another account's picture.
pub(super) async fn upload_for(
    state: App,
    session: Extension<SessionHandle>,
    Extension(editor): Extension<AuthenticatedUser>,
    Path(target): Path<i64>,
    upload: VerifiedUpload,
) -> Response {
    store(state, session, editor, target, upload).await
}

async fn store(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    editor: AuthenticatedUser,
    target: i64,
    mut upload: VerifiedUpload,
) -> Response {
    if !may_change(&app, &editor, target).await {
        return forbidden();
    }
    let ctx = &app.ctx;
    let mut picture: Option<(String, Vec<u8>)> = None;
    let mut too_large = false;
    while let Ok(Some(mut field)) = upload.next_field().await {
        if field.name().unwrap_or_default() != "file" {
            continue;
        }
        let name = field
            .file_name()
            .map(str::to_string)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "picture".to_string());
        let mut bytes: Vec<u8> = Vec::new();
        while let Ok(Some(chunk)) = field.chunk().await {
            if bytes.len() as u64 + chunk.len() as u64 > LIMIT {
                too_large = true;
                break;
            }
            bytes.extend_from_slice(&chunk);
        }
        if !too_large && !bytes.is_empty() {
            picture = Some((name, bytes));
        }
    }
    if too_large {
        session.push_flash(FlashLevel::Error, t!("A picture can be at most 5 MB."));
        return back(&app, &editor, target);
    }
    let Some((name, bytes)) = picture else {
        session.push_flash(FlashLevel::Error, t!("Choose a picture first."));
        return back(&app, &editor, target);
    };
    let prepared = admit(bytes).map(|b| laterite_media::image::prepare(bytes::Bytes::from(b)));
    let Some(prepared) = prepared.filter(|p| p.width.is_some() && p.height.is_some()) else {
        session.push_flash(
            FlashLevel::Error,
            t!("That file is not a picture this panel can read."),
        );
        return back(&app, &editor, target);
    };
    let driver = app.disks.library.as_ref();
    let ingested = match ingest(driver, stream_of(prepared.bytes.clone()), LIMIT).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(error = %e, "an account picture could not be stored");
            session.push_flash(FlashLevel::Error, t!("Could not be stored."));
            return back(&app, &editor, target);
        }
    };
    let created = record::create_with(
        ctx.db(),
        NewMedia {
            disk: DISK.to_string(),
            hash: ingested.hash.clone(),
            size: ingested.size as i64,
            content_type: ingested.content_type,
            original_name: name,
            in_library: false,
            created_by: Some(editor.user.id),
        },
        NewMediaExtra {
            folder_id: None,
            width: prepared.width.map(i64::from),
            height: prepared.height.map(i64::from),
            meta: None,
        },
    )
    .await;
    let saved = match created {
        Ok(id) => match record::find(ctx.db(), id).await {
            Ok(Some(r)) => keep(&app, target, Some((id, variant_url(&r, "thumb")))).await,
            _ => false,
        },
        Err(e) => {
            tracing::error!(error = %e, "an account picture could not be recorded");
            false
        }
    };
    if saved {
        ctx.audit(
            &editor,
            "backend.user.avatar",
            Some("backend_user"),
            Some(&target.to_string()),
            None,
            None,
        )
        .await;
        session.push_flash(FlashLevel::Success, t!("Picture saved."));
    } else {
        session.push_flash(FlashLevel::Error, t!("Could not be stored."));
    }
    back(&app, &editor, target)
}

/// Removes your own picture.
pub(super) async fn remove_own(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(editor): Extension<AuthenticatedUser>,
) -> Response {
    let target = editor.user.id;
    remove(&app, &session, &editor, target).await
}

/// Removes another account's picture.
pub(super) async fn remove_for(
    State(app): App,
    Extension(session): Extension<SessionHandle>,
    Extension(editor): Extension<AuthenticatedUser>,
    Path(target): Path<i64>,
) -> Response {
    remove(&app, &session, &editor, target).await
}

async fn remove(
    app: &super::AppState,
    session: &SessionHandle,
    editor: &AuthenticatedUser,
    target: i64,
) -> Response {
    if !may_change(app, editor, target).await {
        return forbidden();
    }
    if keep(app, target, None).await {
        app.ctx
            .audit(
                editor,
                "backend.user.avatar_remove",
                Some("backend_user"),
                Some(&target.to_string()),
                None,
                None,
            )
            .await;
        session.push_flash(FlashLevel::Success, t!("Picture removed."));
    } else {
        session.push_flash(FlashLevel::Error, t!("Could not save. Please try again."));
    }
    back(app, editor, target)
}

/// Makes `picture` (a media id and its address) the account's, or leaves it
/// with none. The link is what keeps the file from the collector; the one it
/// replaces is left for the collector to take.
async fn keep(app: &super::AppState, target: i64, picture: Option<(i64, String)>) -> bool {
    let db = app.ctx.db();
    let owner_id = target.to_string();
    let owner = Owner {
        owner_type: OWNER,
        owner_id: &owner_id,
        field: "avatar",
    };
    let (id, url) = match &picture {
        Some((id, url)) => (Some(*id), url.as_str()),
        None => (None, ""),
    };
    let ids: Vec<i64> = id.into_iter().collect();
    let done = async {
        link::set(db, owner, &ids)
            .await
            .map_err(|e| e.to_string())?;
        laterite_auth::store::set_user_avatar(db, target, id)
            .await
            .map_err(|e| e.to_string())?;
        laterite_auth::store::set_user_preference(db, target, PREFERENCE, url)
            .await
            .map_err(|e| e.to_string())
    }
    .await;
    if let Err(e) = &done {
        tracing::error!(error = %e, user = target, "an account's picture could not be changed");
    }
    done.is_ok()
}
