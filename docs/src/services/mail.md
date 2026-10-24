# Mail

A message names a template and its variables, or carries its own subject and body; `send` queues it and a worker delivers it through the configured driver.

```rust
use laterite_core::mail::Message;

ctx.mail()
    .send(
        Message::template("acme.blog::welcome", serde_json::json!({ "name": "Ada", "link": url }))
            .to("Ada <ada@acme.example>"),
    )
    .await?;
```

Method | Does
--- | ---
`Message::template(code, vars)` | A message rendered from the template `code`.
`Message::raw(subject, text, html)` | A message with its own body; `html` optional.
`.to(addr)`, `.cc(addr)`, `.bcc(addr)`, `.reply_to(addr)`, `.from(addr)` | Addresses, as `Name <address>` or bare.
`.attach(filename, content_type, bytes)` | A file.
`mail.send(message)` | Queues it; `Pushed::Ran` in inline mode.
`mail.send_now(message)` | Renders and delivers here, returning the driver's error.
`mail.envelope(message)` | What the driver would deliver, rendered and addressed; nothing is sent.
`mail.send_test(addr)` | The built-in test template, delivered here.

## Register a template

```rust
use laterite_core::mail::MailRegistry;

fn register(&self, registry: &mut Registry) {
    registry.add_mail_template("acme.blog::welcome", include_str!("../mail/welcome.htm"));
    registry.add_mail_layout("acme", include_str!("../mail/layouts/acme.htm"));
    registry.add_mail_partial("signature", include_str!("../mail/partials/signature.htm"));
}
```

```text
subject = "Welcome, {{ name }}"
layout = "default"
description = "Sent when an account is made."
==
Welcome {{ name }}.

{{ partial("button", url=link, body="Sign in") }}
==
# Hello {{ name }}

Your account on **{{ app_name }}** is ready.

{% set body %}Sign in{% endset %}
{{ partial("button", url=link, body=body) }}
```

Section | Holds
--- | ---
Header | `subject`, `layout` (`default` unless set), `description`; a layout or partial has `name`.
First `==` | The text body.
Second `==` | The HTML body, written as Markdown.
Third `==` | A layout's CSS, which reads the brand as `{{ brand.button_bg }}`.

A layout receives `content`, `subject`, `app_name`, `css` and `brand_css`. An undefined variable renders empty. A template edited in the admin keeps its edit until it is reset to the file.

Template | Sent by
--- | ---
`laterite::test` | The Mail settings' Send test and `lat mail test`.
`laterite.auth::invite` | Creating an operator; `name`, `username`, `link`, `expires`.
`laterite.auth::reset` | The Forgot your password page; `name`, `username`, `link`, `expires`.

Partial | Arguments
--- | ---
`button` | `url`, `body`
`panel`, `table`, `subcopy` | `body`, Markdown
`header`, `footer` | `body`, `url` (header only)

## Configure the transport

```toml
# config/default.toml
[mail]
driver = "smtp"            # smtp | log | stub | a registered driver
from = "Acme <no-reply@acme.example>"
# to = "dev@acme.example"  # every message goes here while set

[mail.smtp]
host = "smtp.acme.example"
port = 587
encryption = "starttls"    # starttls | tls | none
username = "apikey"
password = ""              # or LAT__MAIL__SMTP__PASSWORD
timeout_seconds = 30
```

Key | Description
--- | ---
`mail.driver` | `smtp` (the `smtp` feature of `laterite-core`, on in `lateritecmf`), `log` (kept in the mail log, nothing leaves), `stub` (kept in memory) or a driver a module registered. Default `log`.
`mail.from` | The sender. Default `Laterite <no-reply@localhost>`.
`mail.to` | A development override: every message goes to this address instead of its recipients.
`mail.smtp.*` | The server, port, encryption, credentials and timeout.

The Mail settings under **Settings → System → Mail** lay over these: a blank field keeps the configured value, the password is sealed under the application key, and a save switches the transport at once. Gated by `backend.manage_mail`.

```bash
lat mail test ada@acme.example
```

**Send a test message to me** on the settings screen mails the signed-in operator the built-in `laterite::test` template and shows the driver's error when it fails.

## Edit templates in the admin

```yaml
# a role's permissions
- backend.manage_mail_templates
- backend.manage_mail
```

Permission | Grants
--- | ---
`backend.manage_mail_templates` | **Settings → System → Mail Templates**: the templates, layouts and partials with a preview, a test send to the signed-in operator, Reset to the file, Delete for one no file registers, and New; **Mail Branding**: the colours every layout reads, with a preview on the test template.
`backend.manage_mail` | **Mail** (the sender and transport, Send a test message) and **Mail Log**: the messages the `log` driver kept, each as text and as HTML, with Clear.

Brand colour | Layout property
--- | ---
Page background, content background, panel background | `brand.body_bg`, `brand.content_bg`, `brand.panel_bg`
Header text, headings, text, muted text, footer text | `brand.header_color`, `brand.heading_color`, `brand.text_color`, `brand.muted_color`, `brand.footer_color`
Links, button, button text, borders | `brand.link_color`, `brand.button_bg`, `brand.button_text_color`, `brand.border_color`

## Register a driver

```rust
use laterite_core::mail::{MailDriverReg, MailRegistry, Mailer};

fn register(&self, registry: &mut Registry) {
    registry.add_mail_driver(MailDriverReg::new("acme-http", |config, _db| {
        Ok(Arc::new(AcmeHttp::new(&config.smtp.username)))
    }));
}
```

A driver implements `Mailer`: a `name()` and `deliver(&Envelope)`; `mail.driver = "acme-http"` or the settings screen selects it.
