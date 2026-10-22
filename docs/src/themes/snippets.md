# Snippets

A snippet is an insertable block in the rich and markdown editors, backed by a partial or by a component.

```json
{
  "snippet": {
    "code": "youtube",
    "name": "YouTube video",
    "description": "Embeds a video by its URL",
    "properties": [
      { "code": "url", "title": "Video URL", "type": "string" },
      { "code": "size", "title": "Size", "type": "dropdown", "options": "sm:Small | lg:Large", "default": "sm" },
      { "code": "autoplay", "title": "Autoplay", "type": "checkbox", "default": false }
    ]
  }
}
```

```twig
{# partials/snippets/youtube.htm #}
<iframe src="https://www.youtube.com/embed/{{ url|replace("https://www.youtube.com/watch?v=", "") }}{% if autoplay %}?autoplay=1{% endif %}" class="video video--{{ size }}"></iframe>
```

```twig
{{ post.body|content }}
```

Key | Values | Default
--- | --- | ---
`code` | letters, digits, `.`, `-`, `_` | required
`name`, `description` | shown in the Insert menu | `description` optional
`properties[].code` | the variable name in the partial | required
`properties[].title` | the dialog's label | required
`properties[].type` | `string`, `dropdown`, `checkbox` | `string`
`properties[].default` | the value when unset | none
`properties[].options` | `value:Label \| value2:Label2` for a dropdown | none

## Declare a snippet on a partial

A partial becomes a snippet through the `snippet` key of its settings, as above. In the HTML an editor saves, an inserted snippet is `<figure data-snippet="youtube" data-properties="{...}"></figure>`; `\|content` renders the partial with each property as a variable beside `this` and `head`. A snippet no partial declares renders as an HTML comment naming it.

## Declare it from the partial page

The partial's page carries the declaration as fields under Snippet.

Field | Writes
--- | ---
Code | `snippet.code`; blank withdraws the snippet.
Name, Description | `snippet.name`, `snippet.description`
Properties | One row per property: Code, Title, Type, Default, Options. A checkbox default of `true`, `1`, `on` or `yes` is stored as `true`.

## Back a snippet with a component

```rust
registry.add(
    ComponentReg::new("acme.map", "Map", Arc::new(Map))
        .snippet("Map", Some("A map centred on an address")),
);
```

```twig
{# partials/acme.map/default.htm, else the component's own markup #}
<div class="map" data-lat="{{ component.lat }}" data-lng="{{ component.lng }}"></div>
```

Step | Does
--- | ---
`.snippet(name, description)` | Lists the component in the Insert menu; its properties are the dialog's fields (`Bool` a checkbox, the rest text).
`\|content` | Leaves the figure in place.
After the page renders | Each distinct figure runs the component with its properties (text read to the declared kinds) and renders `partials/<code>/default.htm`, else the component's markup, with `component`, the code, `this` and `head`.
A failure | An HTML comment naming the snippet.

The component's properties and markup are described under [Components](components.md).
