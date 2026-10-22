# Tags

A tag is a `{% %}` statement: a page extends a layout, fills its blocks, includes a partial, loops and branches.

```jinja
{% extends "layouts/default.htm" %}
{% block page %}
  {% include "partials/nav.htm" %}
  {% for item in latest.items %}<li>{{ item.title }}</li>{% endfor %}
  {% if this.preview %}<p>Previewing.</p>{% endif %}
{% endblock %}
```

Tag | Gives
--- | ---
`{% extends "layouts/<path>" %}` | A page inside a layout.
`{% block <name> %}...{% endblock %}` | A block the layout defines and the page fills; `{% block page %}` in a layout is where the page renders.
`{% include "partials/<path>" %}` | A partial with the current variables.
`{% for item in items %}...{% endfor %}` | One pass per item.
`{% if value %}...{% endif %}` | A branch; an undefined value is false.

## Fill a layout from a page

```jinja
{% block title %}{{ this.page.title }}{% endblock %}
<h1>{{ this.page.title }}</h1>
```

A page whose settings name a `layout` fills the layout's `page` block, and a block the page defines at its top level fills the layout's block of that name without rendering in place. A page that writes `{% extends %}` itself is left as written (see [Layouts and Partials](../themes/layouts-and-partials.md)).
