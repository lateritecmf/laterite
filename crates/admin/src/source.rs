//! Resources whose records live as files: the wiring between a resource's
//! `source` and [`laterite_core::source`].
//!
//! Boot calls [`sync`]: every sourced resource's declaration is checked
//! against its form, its folder is read into its table, and a listener is
//! returned that writes each saved record's file back and removes a deleted
//! record's. A resource with no `source` is untouched.

use std::path::Path;
use std::sync::Arc;

use laterite_core::source::{Column, Columns, FilesListener, Shape};
use laterite_core::{Db, ModelListenerReg};

use crate::field::{FieldRegistry, FieldTypeReg};
use crate::form::FormConfig;
use crate::Resource;

/// The stored columns of a form, in its order, each with the shape its
/// file spells it in: a switch is `true`, a number input is a number, a
/// repeater or checklist is structured data, anything else text. A type
/// the registry does not know stores text.
pub fn columns(form: &FormConfig, types: &FieldRegistry) -> Columns {
    let stored = form
        .fields
        .iter()
        .filter(|f| !f.transient)
        .filter(|f| types.get(&f.field_type).is_none_or(|t| t.stores()))
        .map(|f| {
            let shape = match f.field_type.as_str() {
                "switch" => Shape::Bool,
                "repeater" | "checklist" => Shape::Json,
                "text" if f.options.get("input").and_then(|v| v.as_str()) == Some("number") => {
                    Shape::Number
                }
                _ => Shape::Text,
            };
            Column::new(&f.name, shape)
        })
        .collect();
    Columns::new(&form.id_field, stored)
}

/// Checks every sourced resource, reads its folder into its table, and
/// returns the listeners that keep the folders in step. A declaration that
/// does not fit, or a file that does not parse, is an error naming it.
pub async fn sync(
    db: &Db,
    root: &Path,
    resources: &[Resource],
    field_types: &[FieldTypeReg],
) -> anyhow::Result<Vec<ModelListenerReg>> {
    let mut types = crate::field::builtin_registry();
    for reg in field_types {
        types.insert(reg.name().to_string(), Arc::clone(&reg.field_type));
    }
    let mut listeners = Vec::new();
    for resource in resources {
        let Some(source) = &resource.source else {
            continue;
        };
        let Some(form) = &resource.form else {
            anyhow::bail!(
                "admin resource `{}`: a source needs a form, since the form's fields are the file's",
                resource.base_path
            );
        };
        let columns = columns(form, &types);
        source
            .validate(&columns)
            .map_err(|e| anyhow::anyhow!("admin resource `{}`: {e}", resource.base_path))?;
        let report = laterite_core::source::pull(db, root, &form.entity, &columns, source)
            .await
            .map_err(|e| anyhow::anyhow!("admin resource `{}`: {e}", resource.base_path))?;
        tracing::info!(
            entity = %form.entity,
            path = %source.path,
            created = report.created,
            updated = report.updated,
            removed = report.removed,
            "records read from files"
        );
        listeners.push(ModelListenerReg::for_entity(
            form.entity.clone(),
            Arc::new(FilesListener::new(
                root,
                form.entity.clone(),
                columns,
                source.clone(),
            )),
        ));
    }
    Ok(listeners)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::FormField;

    #[test]
    fn columns_follow_the_form_and_shape_by_type() {
        let form = FormConfig::new(
            "events",
            "Event",
            "/events",
            "id",
            vec![
                FormField::text("slug", "Slug"),
                FormField::text("title", "Title"),
                FormField::switch("pinned", "Pinned"),
                FormField::text("capacity", "Capacity")
                    .options(serde_json::json!({ "input": "number" })),
                FormField::of("about", "About", "section"),
                FormField::repeater("tracks", "Tracks", vec![FormField::text("name", "Name")]),
                FormField::text("secret", "Secret").transient(),
            ],
        );
        let columns = columns(&form, &crate::field::builtin_registry());
        assert_eq!(columns.id, "id");
        let names: Vec<&str> = columns.stored.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["slug", "title", "pinned", "capacity", "tracks"]);
        assert_eq!(columns.stored[2].shape, Shape::Bool);
        assert_eq!(columns.stored[3].shape, Shape::Number);
        assert_eq!(columns.stored[4].shape, Shape::Json);
    }
}
