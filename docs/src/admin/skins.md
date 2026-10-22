# Skins

A skin is a stylesheet layered after the framework's own, selected per
deployment by `backend.skin`.

## Select a skin

```toml
# config/default.toml
[backend]
skin = "compact"
```

Code | Ships with | Stylesheet
--- | --- | ---
`standard` | the framework | none
`compact` | the framework | tighter spacing, smaller controls, a narrower sidebar

## Contribute a skin

```rust
use laterite_admin::skin::Skin;
use laterite_admin::{AdminAsset, AdminRegistry};
use laterite_core::t;

fn register(registry: &mut laterite_core::Registry) {
    registry.add_asset(
        "acme/dense.css",
        AdminAsset::css(include_bytes!("../assets/dense.css")),
    );
    registry.add_skin(Skin {
        code: "acme.dense",
        label: t!("Dense"),
        stylesheet: Some("acme/dense.css"),
    });
}
```

```css
/* assets/dense.css */
:root[data-lat-skin="acme.dense"] {
  --card-pad: 10px;
  --control-h: 32px;
  --sidebar-w: 220px;
}
[data-lat-skin="acme.dense"] .lat-table td {
  padding: 6px 10px;
}
```

Field | Value
--- | ---
`code` | Selected by `backend.skin`. A module's carries its vendor: `acme.dense`.
`label` | The skin's name.
`stylesheet` | The asset key to load after `laterite.css`, or `None`.

`add_asset(key, asset)` serves a compiled-in file at `{admin}/assets/{key}`,
content-named and cached for a year.

Constructor | Media type
--- | ---
`AdminAsset::css(bytes)` | `text/css`
`AdminAsset::js(bytes)` | `text/javascript`
`AdminAsset::new(mime, bytes)` | as given

## Set the tokens

The document element carries `data-lat-skin="<code>"` on every admin page,
the sign-in page and the setup page. A skin's rules key on it.

```css
:root[data-lat-skin="acme.dense"] {
  --topnav-h: 54px;
  --sidebar-w: 230px;
  --side-panel-w: 300px;
  --page-gutter: 20px;
  --card-pad: 14px;
  --control-h: 34px;
  --control-h-sm: 28px;
  --control-h-lg: 40px;
  --radius-control: 6px;
  --radius-card: 6px;
}
```

Token | Sets
--- | ---
`--topnav-h` | Top bar height
`--sidebar-w` | Settings sidebar width
`--side-panel-w` | Context side panel width
`--page-gutter` | Page padding
`--card-pad` | Card padding
`--control-h`, `--control-h-sm`, `--control-h-lg` | Input and button heights
`--radius-control`, `--radius-card` | Corner radii

The brand's accent and custom stylesheet load after the skin.

An unknown `backend.skin` code, a skin naming an unregistered stylesheet key,
a duplicate skin code, or an asset key already served all abort the start.
