//! The deployment's skin reaches every page: the document element carries its
//! code and its stylesheet is linked after the framework's. Imports only the
//! framework's public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::skin::Skin;
use laterite_admin::{router, AdminAsset, AdminConfig, AssetReg, Contributions};
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::i18n::Text;
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

/// The admin router with a skin selected, mounted at `/admin`.
fn app(db: Db, skin: &str, contributions: Contributions) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    let mut config = AdminConfig::default();
    config.skin = skin.to_string();
    router(
        auth,
        db,
        contributions,
        config,
        Arc::new(CatalogStore::default()),
    )
}

/// GETs a path: the status, the content type and the body. A fresh install
/// serves the first-run setup page, which renders through the pre-auth chrome.
async fn get(router: Router, path: &str) -> (StatusCode, String, String) {
    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let kind = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, kind, String::from_utf8(bytes.to_vec()).unwrap())
}

/// The href of the first stylesheet link under `prefix`, if any.
fn stylesheet_under(html: &str, prefix: &str) -> Option<String> {
    html.split("<link rel=\"stylesheet\" href=\"")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap_or_default().to_string())
        .find(|href| href.starts_with(prefix))
}

#[tokio::test]
async fn the_standard_skin_marks_the_document_and_loads_nothing_extra() {
    let (db, _guard) = test_db().await;
    let (status, _, html) = get(
        app(db, "standard", Contributions::default()),
        "/admin/setup",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-lat-skin=\"standard\""), "{html}");
    assert!(stylesheet_under(&html, "/admin/assets/skins/").is_none());
}

#[tokio::test]
async fn the_compact_skin_links_its_stylesheet_and_serves_it() {
    let (db, _guard) = test_db().await;
    let (_, _, html) = get(
        app(db.clone(), "compact", Contributions::default()),
        "/admin/setup",
    )
    .await;
    assert!(html.contains("data-lat-skin=\"compact\""), "{html}");
    let href = stylesheet_under(&html, "/admin/assets/skins/compact.").expect("skin stylesheet");
    assert!(href.ends_with(".css"), "{href}");
    let (status, kind, css) = get(app(db, "compact", Contributions::default()), &href).await;
    assert_eq!(status, StatusCode::OK);
    assert!(kind.starts_with("text/css"), "{kind}");
    assert!(css.contains("[data-lat-skin=\"compact\"]"));
}

fn dense() -> Contributions {
    Contributions {
        skins: vec![Skin {
            code: "acme.dense",
            label: Text::new("Dense"),
            stylesheet: Some("acme/dense.css"),
        }],
        assets: vec![AssetReg {
            key: "acme/dense.css",
            asset: AdminAsset::css(b":root[data-lat-skin=\"acme.dense\"]{--card-pad:8px}"),
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn a_contributed_skin_is_selectable_by_code() {
    let (db, _guard) = test_db().await;
    let (_, _, html) = get(app(db.clone(), "acme.dense", dense()), "/admin/setup").await;
    assert!(html.contains("data-lat-skin=\"acme.dense\""), "{html}");
    let href = stylesheet_under(&html, "/admin/assets/acme/dense.").expect("skin stylesheet");
    let (status, kind, css) = get(app(db, "acme.dense", dense()), &href).await;
    assert_eq!(status, StatusCode::OK);
    assert!(kind.starts_with("text/css"), "{kind}");
    assert!(css.contains("--card-pad:8px"));
}

#[tokio::test]
#[should_panic(expected = "unknown admin skin `dense`")]
async fn an_unregistered_code_aborts_the_boot() {
    let (db, _guard) = test_db().await;
    let _ = app(db, "dense", Contributions::default());
}

#[tokio::test]
#[should_panic(expected = "duplicate admin asset `laterite.css`")]
async fn an_asset_shadowing_the_framework_aborts_the_boot() {
    let (db, _guard) = test_db().await;
    let contributions = Contributions {
        assets: vec![AssetReg {
            key: "laterite.css",
            asset: AdminAsset::css(b""),
        }],
        ..Default::default()
    };
    let _ = app(db, "standard", contributions);
}
