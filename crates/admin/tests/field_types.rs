//! Boot-time assembly of the field-type registry. Imports only the public API.
//!
//! The reference field cannot come from the arg-free built-ins: it needs the
//! picker registry, so boot inserts it into the registry separately. Anything
//! that rebuilds the registry after that point silently drops it, and every form
//! using a reference field then fails to boot.

use laterite_admin::form::{FormConfig, FormField};
use laterite_admin::list::{ListColumn, ListConfig};
use laterite_admin::picker::{PickerError, PickerNode, PickerSource, PickerSourceReg};
use laterite_admin::{router, AdminConfig, Contributions, Resource};
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::strata::async_trait;
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;

const SOURCE: &str = "acme.places";

struct Places;

#[async_trait]
impl PickerSource for Places {
    async fn search(
        &self,
        _db: &Db,
        _q: &str,
        _limit: u32,
    ) -> Result<Vec<PickerNode>, PickerError> {
        Ok(Vec::new())
    }
    async fn resolve(&self, _db: &Db, _id: &str) -> Result<Option<PickerNode>, PickerError> {
        Ok(None)
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

/// Booting panics when a form names a field type the registry has not got, so
/// building the router is itself the assertion.
#[tokio::test]
async fn the_reference_field_survives_registry_assembly() {
    let (db, _guard) = test_db().await;
    let auth = AuthService::new(db.clone(), AuthConfig::default());

    let resource = Resource::new(
        "/places",
        "Places",
        ListConfig::new("places", "Places", vec![ListColumn::new("name", "Name")]),
    )
    .form(FormConfig::new(
        "places",
        "Place",
        "/places",
        "id",
        vec![
            FormField::text("name", "Name"),
            FormField::reference("parent_id", "Parent", SOURCE),
        ],
    ))
    .permission("acme.places.manage");

    let _router = router(
        auth,
        db,
        Contributions {
            resources: vec![resource],
            picker_sources: vec![PickerSourceReg::new(SOURCE, Arc::new(Places))],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    );
}
