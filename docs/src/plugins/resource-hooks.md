# Resource Features and Hooks

A module mounts routes of its own under any resource that opts in, or changes
another module's resource before it mounts.

## Mount a feature under a resource

```rust
use laterite_admin::axum::routing::get;
use laterite_admin::axum::Router;
use laterite_admin::routes::{ResourceFeature, ResourceFeatureReg, ResourceInfo, RouteCtx};

struct Archive;

impl ResourceFeature for Archive {
    fn mount(&self, ctx: &RouteCtx, resource: &ResourceInfo) -> Router {
        let form = resource.form.clone();
        Router::new().route("/", get(move || async move { form.entity().to_string() }))
    }
}

registry.add(ResourceFeatureReg::new("archive", Arc::new(Archive)));
```

```yaml
# descriptors/products.yaml
archive: true
```

Field | Meaning
--- | ---
`resource.path` | The resource's path as declared, relative to the admin root (`/products`).
`resource.base_path` | Where its list mounted (`/admin/products`).
`resource.label` | Its menu label.
`resource.permission` | The permission the feature's routes are gated by.
`resource.form` | Its form, to write records through.

The routes mount at `{base_path}/{key}` behind the permission. A descriptor
declaring a key nobody registered aborts boot naming the resource. The admin
knows the `import` key; a new key needs the admin to carry it.

## Change another module's resource

```rust
registry.add_resource_hook(ResourceHook::new("/acme/blog/posts", |mut resource| {
    resource.list.columns.push(ListColumn::new("reviewed", "Reviewed").boolean());
    resource
        .relation(RelationConfig::has_many("notes", "Note", "post_notes", "post_id", notes_list, notes_form))
        .menu(Placement::new().under("acme.content"))
}));
```

Key | Does
--- | ---
`target` | The resource's path as it mounts: `/posts` for an application's, `/acme/blog/posts` for a plugin's under its module base. One nobody declared aborts boot.
The closure | Takes the resource as declared, returns it changed. Anything the builders set: columns, fields, relations, import, the palette source, the menu placement.
Order | Hooks run in module order, before the resource mounts; each sees the previous one's result.
