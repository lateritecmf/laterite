//! Reading a resource from a YAML file.
//!
//! One file describes one [`Resource`]: what it is, where it mounts, who may
//! reach it, and the list and form screens over it.
//!
//! ```yaml
//! entity: posts
//! path: /posts
//! title: Posts
//! permission: acme.blog.manage_posts
//! list:
//!   order_by: created_at
//!   order_dir: desc
//!   creatable: true
//!   columns:
//!     title:      { searchable: true }
//!     status:     { type: status_pill, width: 10% }
//!     created_at: { type: datetime, label: Created }
//! form:
//!   title: Post
//!   fields:
//!     title:  { rules: [required, { max_length: 120 }] }
//!     status: { type: select, options: { options: { draft: Draft } } }
//! ```
//!
//! The entity, the path and the id column are written once at the top and
//! filled into both screens, so the two cannot disagree about what they edit.
//! A key the framework does not know is an error rather than a shrug: a typo in
//! a data-authored admin is otherwise invisible until someone notices the
//! screen is wrong.

use serde::Deserialize;

use crate::form::FormConfig;
use crate::list::ListConfig;
use crate::Resource;
use laterite_core::Text;

/// A resource as a descriptor file spells it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceFile {
    /// The table the screens read and write.
    entity: String,
    /// Where the resource mounts, under the module's own namespace.
    path: String,
    /// The navigation label, and the default title for both screens.
    title: Text,
    /// Required once the resource has a form: an unguarded write aborts boot.
    #[serde(default)]
    permission: Option<String>,
    /// The primary key column.
    #[serde(default = "default_id_field")]
    id_field: String,
    list: ListConfig,
    #[serde(default)]
    form: Option<FormConfig>,
}

fn default_id_field() -> String {
    "id".to_string()
}

/// What went wrong reading a descriptor file.
#[derive(Debug)]
pub struct DescriptorError {
    /// The file, as the caller named it.
    pub source: String,
    pub message: String,
}

impl std::fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.source, self.message)
    }
}

impl std::error::Error for DescriptorError {}

/// Reads a resource from the contents of a descriptor file.
///
/// `source` names the file in any error, so a message points at something the
/// author can open.
pub fn from_yaml(yaml: &str, source: &str) -> Result<Resource, DescriptorError> {
    let file: ResourceFile = serde_saphyr::from_str(yaml).map_err(|e| DescriptorError {
        source: source.to_string(),
        message: e.to_string(),
    })?;

    let mut list = file.list;
    list.entity = file.entity.clone();
    list.id_field = file.id_field.clone();
    if list.title.source().is_empty() {
        list.title = file.title.clone();
    }
    // Rows link to the form when there is one, so a file need not repeat the
    // path it already gave.
    if file.form.is_some() && list.edit_base.is_none() {
        list.edit_base = Some(file.path.clone());
    }
    for column in &mut list.columns {
        if column.label.source().is_empty() {
            column.label = humanize(&column.field);
        }
    }
    for filter in &mut list.filters {
        if filter.label.source().is_empty() {
            filter.label = humanize(&filter.field);
        }
    }

    let mut resource = Resource::new(file.path.clone(), file.title.clone(), list);
    if let Some(permission) = file.permission {
        resource = resource.permission(permission);
    }
    if let Some(mut form) = file.form {
        form.entity = file.entity;
        form.id_field = file.id_field;
        form.base_path = file.path;
        if form.title.source().is_empty() {
            form.title = file.title;
        }
        for field in &mut form.fields {
            if field.label.source().is_empty() {
                field.label = humanize(&field.name);
            }
        }
        resource = resource.form(form);
    }
    Ok(resource)
}

/// `created_at` reads as "Created at" when a file names no label. Still a
/// `Text`, so it localizes and `lat i18n extract` finds it like any other.
fn humanize(name: &str) -> Text {
    let mut out = String::with_capacity(name.len());
    for (i, part) in name.split(['_', '-']).enumerate() {
        if part.is_empty() {
            continue;
        }
        if i > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            if i == 0 {
                out.extend(first.to_uppercase());
            } else {
                out.push(first);
            }
            out.push_str(chars.as_str());
        }
    }
    Text::dynamic(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POSTS: &str = r#"
entity: posts
path: /posts
title: Posts
permission: acme.manage_posts
list:
  creatable: true
  order_by: created_at
  order_dir: desc
  columns:
    title:      { searchable: true }
    created_at: { type: datetime }
form:
  fields:
    title: { rules: [required] }
"#;

    #[test]
    fn a_file_describes_one_resource() {
        let resource = from_yaml(POSTS, "posts.yaml").expect("reads");
        assert_eq!(resource.base_path, "/posts");
        assert_eq!(resource.permission.as_deref(), Some("acme.manage_posts"));
        assert_eq!(resource.list.entity, "posts");
        assert_eq!(resource.list.order_by, "created_at");
        assert!(resource.list.creatable);
    }

    /// The entity, id column and path are written once and reach both screens,
    /// so a file cannot say one thing to the list and another to the form.
    #[test]
    fn the_resource_level_keys_fill_both_screens() {
        let resource = from_yaml(POSTS, "posts.yaml").expect("reads");
        let form = resource.form.as_ref().expect("a form");
        assert_eq!(form.entity, "posts");
        assert_eq!(form.base_path, "/posts");
        assert_eq!(form.id_field, "id");
        assert_eq!(resource.list.id_field, "id");
        assert_eq!(resource.list.edit_base.as_deref(), Some("/posts"));
    }

    #[test]
    fn a_column_is_named_by_its_key_and_labelled_from_it() {
        let resource = from_yaml(POSTS, "posts.yaml").expect("reads");
        let names: Vec<&str> = resource
            .list
            .columns
            .iter()
            .map(|c| c.field.as_str())
            .collect();
        assert_eq!(names, ["title", "created_at"], "in the order written");
        assert_eq!(resource.list.columns[1].label.source(), "Created at");
        assert!(resource.list.columns[0].is_searchable());
    }

    #[test]
    fn a_key_the_framework_does_not_know_is_refused() {
        let yaml = POSTS.replace("searchable: true", "searchabel: true");
        let err = from_yaml(&yaml, "posts.yaml")
            .err()
            .expect("a typo is an error");
        assert_eq!(err.source, "posts.yaml");
        assert!(
            err.message.contains("searchabel"),
            "and names it: {}",
            err.message
        );
    }

    #[test]
    fn a_missing_required_key_is_refused() {
        let yaml = POSTS.replace("entity: posts\n", "");
        let err = from_yaml(&yaml, "posts.yaml")
            .err()
            .expect("entity is required");
        assert!(err.message.contains("entity"), "{}", err.message);
    }

    /// Writing the descriptor back out gives the file it came from, so the
    /// format is a faithful projection of the structs rather than a lossy one.
    #[test]
    fn a_descriptor_round_trips_through_the_file_format() {
        let resource = from_yaml(POSTS, "posts.yaml").expect("reads");
        let written = serde_saphyr::to_string(&resource.list).expect("writes");
        let read_back: ListConfig = serde_saphyr::from_str(&written).expect("reads back");
        assert_eq!(
            read_back
                .columns
                .iter()
                .map(|c| (c.field.clone(), c.column_type.clone()))
                .collect::<Vec<_>>(),
            resource
                .list
                .columns
                .iter()
                .map(|c| (c.field.clone(), c.column_type.clone()))
                .collect::<Vec<_>>(),
            "columns survive, named and in order"
        );
        assert_eq!(read_back.order_by, resource.list.order_by);
        assert_eq!(read_back.creatable, resource.list.creatable);
    }

    #[test]
    fn a_file_may_describe_a_list_with_no_form() {
        let yaml = POSTS.split("form:").next().unwrap();
        let resource = from_yaml(yaml, "posts.yaml").expect("reads");
        assert!(resource.form.is_none());
        assert!(resource.list.edit_base.is_none(), "and links no rows");
    }
}
