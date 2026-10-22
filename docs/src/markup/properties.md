# Properties

Every page, layout and partial reads the request as `this`, the page's head tags as `head`, and the theme as `theme`.

```jinja
<html>
<head>{{ head }}</head>
<body>
  <h1>{{ this.page.title }}</h1>
  <p>{{ this.params.slug }} on {{ this.url }} in {{ this.theme.name }}</p>
  {% if this.preview %}<p>Previewing.</p>{% endif %}
  {% block page %}{% endblock %}
</body>
</html>
```

Name | Gives
--- | ---
`this.page` | The page's settings and `name` (see [Pages](../themes/pages.md)).
`this.params` | The URL's parameters.
`this.theme` | `code` and `name`.
`this.layout` | The layout the page named.
`this.url` | The request path.
`this.preview` | Whether an operator is previewing.
`head` | The title, description, canonical, Open Graph and robots tags from the page's settings; a setting may carry an expression.
`theme` | `code`, `name`, `author`, `homepage`, `description` of the theme rendered.
`theme.<field>` | A value of the theme's own [settings](../themes/theme-settings.md).

## Read a component's data

```jinja
<h1>{{ post.items[0].title }}</h1>
<footer>{{ site_name }}</footer>
```

Name | Gives
--- | ---
`<alias>` | The data a [component](../themes/components.md) attached under that alias returned.
`<name>` | A value a component injected into the page scope.
`component` | Inside a component's markup, its own data.
