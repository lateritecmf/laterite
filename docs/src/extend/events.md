# Events

An event is a fact a module announces. A listener acts on it.

## Listen

```rust
use laterite_auth::events::SignedIn;
use laterite_core::{EventCx, EventError, Listener, Registry};

struct WelcomeBack;

#[laterite_core::strata::async_trait]
impl Listener<SignedIn> for WelcomeBack {
    async fn handle(&self, cx: &EventCx<'_>, event: &SignedIn) -> Result<(), EventError> {
        acme::greetings::record(cx.db(), event.user_id).await?;
        Ok(())
    }
}

fn register(&self, registry: &mut Registry) {
    registry.listen::<SignedIn>(WelcomeBack);
}
```

In a listener | Is
--- | ---
`event` | The payload, typed.
`cx.db()` | The application's pool.
`cx.events()` | The bus, for announcing a fact of the listener's own.
`Err(..)` | Logged with the event and the listening module. The listeners after it still run, and the announcer is not told.

Listeners run one after another in module dependency order, then the order a
module registered them, inside the call that announced.

## Announce

```rust
use laterite_core::Event;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[non_exhaustive]
pub struct Published {
    pub article_id: i64,
}

impl Published {
    pub fn new(article_id: i64) -> Self {
        Self { article_id }
    }
}

impl Event for Published {
    const NAME: &'static str = "acme.blog.published";
}
```

```rust
impl Screen for Articles {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let events = ctx.events().clone();
        let list = ctx.url("/");
        Router::new().route("/{id}/publish", post(move |Path(id): Path<i64>| {
            let (events, list) = (events.clone(), list.clone());
            async move {
                // ... write and commit, then:
                events.emit(&Published::new(id)).await;
                Redirect::to(&list)
            }
        }))
    }
}
```

Part | Rule
--- | ---
`NAME` | The module's id, then the fact in the past tense: `acme.blog.published`. Two types under one name stop the boot, naming both.
Payload | Serde in both directions. `#[non_exhaustive]` with a constructor, so a field added later breaks no listener.
Moment | After the write has committed.

Where | The bus
--- | ---
A screen or a public route | `ctx.events()` on `RouteCtx`
Routes added with `Bootstrap::extend` | `ctx.events()` on `BootstrapCtx`
A listener | `cx.events()`
Beside an `AuthService` | `auth.events()`

## Framework events

Types are in `laterite_auth::events`.

Name | Type | Fields
--- | --- | ---
`auth.signed_in` | `SignedIn` | `user_id`, `username`, `ip_address`, `user_agent`
`auth.sign_in_failed` | `SignInFailed` | `user_id` (none for an unknown username), `username`, `ip_address`, `user_agent`
`auth.locked_out` | `LockedOut` | `user_id`, `username`, `ip_address`, `user_agent`
`auth.signed_out` | `SignedOut` | `user_id`
`auth.password_changed` | `PasswordChanged` | `user_id`, `changed_by` (none for a process); `by_owner()`

Record writes are heard through [Model Listeners](model-listeners.md), which
can also refuse one.

## Test

```rust
let events = Events::builder(db.clone())
    .listen::<SignedIn>(WelcomeBack)
    .build()?;

// A listener, alone.
events.emit(&SignedIn::new(7, "ada", &RequestContext::default())).await;

// The service announcing to it.
let auth = AuthService::new(db.clone(), AuthConfig::default()).with_events(events.clone());

// A route announcing to it.
let ctx = RouteCtx::builder(db).events(events).build();
let response = Articles.mount(&ctx).oneshot(request).await?;
```

## Not built yet

- Refusing an action from a listener.
- Listening by a wildcard name.
- Queued delivery.
