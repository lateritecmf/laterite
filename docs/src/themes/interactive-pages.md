# Interactive Pages

A page posts to a component's handler and swaps the answer into place; the component declares its handlers, the template attaches them.

```rust
use laterite_web::theme::component::{Component, ComponentContext, ComponentOutput, Handled, HandlerContext};

#[async_trait]
impl Component for Todo {
    async fn run(&self, cx: &ComponentContext<'_>) -> Result<ComponentOutput, ThemeError> {
        Ok(ComponentOutput::data(json!({ "items": load(cx.db).await? })))
    }
    fn handlers(&self) -> &'static [&'static str] {
        &["add"]
    }
    async fn handle(&self, name: &str, cx: &HandlerContext<'_>) -> Result<Handled, ThemeError> {
        let mut items = cx.input.list("items");
        let item = cx.input.text("new_item").to_string();
        if item.is_empty() {
            return Ok(Handled::invalid("new_item", "Say what to add.").with_data(json!({ "items": items })));
        }
        if items.len() as u64 >= cx.property_u64("max", 10) {
            return Ok(Handled::invalid("new_item", "The list is full."));
        }
        items.push(item);
        Ok(Handled::data(json!({ "items": items })).inject("saved", true))
    }
}
```

```jinja
{{ framework() }}
<form {{ request('todo::add', target='#todo') }}>
  {{ token('todo::add') }}
  {% for item in todo.items %}<input type="hidden" name="items[]" value="{{ item }}">{% endfor %}
  <input name="new_item">
  <button>Add</button>
</form>
<div id="todo">{{ component('todo') }}</div>
```

```jinja
{# partials/todo/default.htm #}
<ul>{% for item in component.items %}<li>{{ item }}</li>{% endfor %}</ul>
{% if errors.new_item %}<p class="error">{{ errors.new_item }}</p>{% endif %}
```

## Declare a handler

Method | Gives
--- | ---
`handlers()` | The names a page may post to. A name not listed is a 404.
`handle(name, cx)` | The outcome for one post, after `run` has produced the component's data.
`cx.input.text(name)` | The first value under `name`, trimmed; empty when absent.
`cx.input.list(name)` | Every value under `name` or `name[]`.
`cx.input.has(name)` | Whether the field was posted.
`cx.property_u64(name, default)` | A number property of the attachment.
`cx.properties`, `cx.params`, `cx.path`, `cx.headers`, `cx.db`, `cx.preview` | The request, as `run` sees it.

## Return an outcome

Outcome | Renders
--- | ---
`Handled::data(value)` | The alias with `value` as its data; `.inject(name, value)` adds a page variable.
`Handled::invalid(field, message)` | Status 422 with `errors.<field>` in scope; `.with_data(value)` sets the alias's data.
`Handled::Redirect(url)` | `HX-Redirect` for HTMX, a 303 otherwise.
`Handled::Json(value)` | The value as `application/json`.
`Handled::Unknown` | A 404.

## Attach it in a template

Function | Emits
--- | ---
`framework()` | The script at `/laterite/htmx.js` and the island that swaps a 422.
`request(handler, target=, swap=, partial=, confirm=, indicator=)` | `hx-post` to the page, `hx-headers` naming the handler and the partial, `hx-vals` with the visitor's token, and the HTMX attributes given.
`token(handler)` | The hidden `_token` and `_handler` fields, for a form that posts without a script.
`component(alias)` | The alias's markup: `partials/<alias>/default.htm`, else the component's own.

`partial` names a theme partial to render in place of the alias's markup, with the page's scope. Without HTMX the whole page renders, at 200 or 422, and a redirect is a 303.

## Read the scope

Name | Holds
--- | ---
`errors` | The messages by field of a refused input; an empty map otherwise.
`laterite_token` | The visitor's token; `request()` and `token()` read it.
`this.url` | The page's URL, where the post goes.

A post from another site, or without the visitor's token, is a 403. The token is a cookie set on the first page and lives a day.
