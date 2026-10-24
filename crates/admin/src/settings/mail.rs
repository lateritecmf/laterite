//! The Mail settings: the sender and the transport, saved over `[mail]`,
//! with a Send test that mails the signed-in operator.

use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;
use laterite_auth::AuthenticatedUser;
use laterite_core::config::MailConfig;
use laterite_core::crypt::{Crypt, FIELD_PURPOSE, MASK};
use laterite_core::t;
use serde::{Deserialize, Serialize};

use super::{SettingsItem, SettingsModel};
use crate::form::FormField;
use crate::session::{FlashLevel, SessionHandle};
use crate::AdminState;

/// The permission the Mail settings and the Send test require.
pub const PERMISSION: &str = "backend.manage_mail";

/// Where the Send test posts, under the admin root.
pub const TEST_PATH: &str = "/settings/mail/test";

/// The saved mail settings. A blank field leaves `[mail]`'s value in force.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailSetting {
    #[serde(default)]
    pub from_name: String,
    #[serde(default)]
    pub from_address: String,
    /// `smtp`, `log`, `stub` or a registered driver; blank keeps the config.
    #[serde(default)]
    pub driver: String,
    #[serde(default)]
    pub smtp_host: String,
    #[serde(default)]
    pub smtp_port: String,
    #[serde(default)]
    pub smtp_encryption: String,
    #[serde(default)]
    pub smtp_username: String,
    /// Sealed by the `sensitive` field type.
    #[serde(default)]
    pub smtp_password: String,
    /// A development override: every message goes here while set.
    #[serde(default)]
    pub to_override: String,
}

impl SettingsModel for MailSetting {
    const CODE: &'static str = "laterite.mail";
}

/// The settings screen under System.
pub fn settings_item() -> SettingsItem {
    SettingsItem::new(
        MailSetting::CODE,
        t!("Mail"),
        vec![
            FormField::text("from_name", t!("Sender name")).help(t!("Shown as the sender; blank keeps the configured name.")),
            FormField::text("from_address", t!("Sender address")).help(t!("Blank keeps the configured address.")),
            FormField::select(
                "driver",
                t!("Send through"),
                vec![("", "Configured driver"), ("smtp", "SMTP"), ("log", "Mail log (nothing leaves)"), ("stub", "Nowhere (tests)")],
            )
            .help(t!("A driver a module registered is named by its code in the configuration.")),
            FormField::text("smtp_host", t!("SMTP host")),
            FormField::text("smtp_port", t!("SMTP port")).help(t!("587 for STARTTLS, 465 for TLS, 25 or 1025 for none.")),
            FormField::select(
                "smtp_encryption",
                t!("Encryption"),
                vec![("", "Configured"), ("starttls", "STARTTLS"), ("tls", "TLS"), ("none", "None")],
            ),
            FormField::text("smtp_username", t!("SMTP username")),
            FormField::of("smtp_password", t!("SMTP password"), "sensitive")
                .options(serde_json::json!({ "reveal": false })),
            FormField::text("to_override", t!("Send everything to")).help(t!(
                "While set, every message goes to this address instead of its recipients. For development."
            )),
        ],
    )
    .description(t!("The sender, the transport and a test message."))
    .hint(t!("Blank fields keep the values from the configuration file. Save, then send a test message to yourself."))
    .category(t!("System"))
    .slot(laterite_core::arrange::slot::SYSTEM)
    .order(14)
    .icon("mail")
    .permission(PERMISSION)
    .action(t!("Send a test message to me"), TEST_PATH)
}

/// The configuration in force: `config` with every non-blank setting laid
/// over it, the password opened with `crypt`.
pub fn effective(config: &MailConfig, setting: &MailSetting, crypt: &Crypt) -> MailConfig {
    let mut out = config.clone();
    let set = |target: &mut String, value: &str| {
        if !value.trim().is_empty() {
            *target = value.trim().to_string();
        }
    };
    if !setting.from_address.trim().is_empty() {
        let name = if setting.from_name.trim().is_empty() {
            laterite_core::mail::Address::parse(&config.from)
                .and_then(|a| a.name)
                .unwrap_or_default()
        } else {
            setting.from_name.trim().to_string()
        };
        out.from = if name.is_empty() {
            setting.from_address.trim().to_string()
        } else {
            format!("{name} <{}>", setting.from_address.trim())
        };
    } else if !setting.from_name.trim().is_empty() {
        if let Some(address) = laterite_core::mail::Address::parse(&config.from) {
            out.from = format!("{} <{}>", setting.from_name.trim(), address.email);
        }
    }
    set(&mut out.driver, &setting.driver);
    set(&mut out.smtp.host, &setting.smtp_host);
    if let Ok(port) = setting.smtp_port.trim().parse::<u16>() {
        out.smtp.port = port;
    }
    set(&mut out.smtp.encryption, &setting.smtp_encryption);
    set(&mut out.smtp.username, &setting.smtp_username);
    let sealed = setting.smtp_password.trim();
    if !sealed.is_empty() && sealed != MASK {
        match crypt.decrypt(FIELD_PURPOSE, b"smtp_password", sealed) {
            Ok(plain) => out.smtp.password = String::from_utf8_lossy(&plain).to_string(),
            Err(_) => {
                tracing::warn!("the saved SMTP password does not open under the current keys")
            }
        }
    }
    if !setting.to_override.trim().is_empty() {
        out.to = Some(setting.to_override.trim().to_string());
    }
    out
}

/// Sends the built-in test template to the signed-in operator.
pub(crate) async fn send_test(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    let back = format!("{}/settings/laterite/mail", state.admin_path);
    let to = user.user.email.clone();
    match state.mail.send_test(&to).await {
        Ok(()) => {
            crate::audit::record(
                &state,
                &user,
                "backend.mail.test",
                Some("mail"),
                Some(&to),
                None,
                Some(state.mail.driver_name()),
            )
            .await;
            session.push_flash(
                FlashLevel::Success,
                laterite_core::t!(
                    "A test message went to {address} through {driver}.",
                    address = to,
                    driver = state.mail.driver_name()
                ),
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "test message failed");
            session.push_flash(
                FlashLevel::Error,
                laterite_core::t!(
                    "The test message could not be sent: {error}",
                    error = e.to_string()
                ),
            );
        }
    }
    Redirect::to(&back).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_lay_over_the_configuration() {
        let crypt = Crypt::ephemeral();
        let config = MailConfig {
            driver: "log".into(),
            from: "Acme <no-reply@acme.example>".into(),
            ..MailConfig::default()
        };
        let sealed = crypt
            .encrypt(FIELD_PURPOSE, b"smtp_password", b"hunter2")
            .unwrap();
        let setting = MailSetting {
            from_name: "Acme Mail".into(),
            driver: "smtp".into(),
            smtp_host: "smtp.acme.example".into(),
            smtp_port: "465".into(),
            smtp_encryption: "tls".into(),
            smtp_username: "u".into(),
            smtp_password: sealed,
            ..MailSetting::default()
        };
        let out = effective(&config, &setting, &crypt);
        assert_eq!(out.from, "Acme Mail <no-reply@acme.example>");
        assert_eq!(out.driver, "smtp");
        assert_eq!(out.smtp.host, "smtp.acme.example");
        assert_eq!(out.smtp.port, 465);
        assert_eq!(out.smtp.encryption, "tls");
        assert_eq!(out.smtp.password, "hunter2");
        assert_eq!(out.to, None);
        let blank = effective(&config, &MailSetting::default(), &crypt);
        assert_eq!(blank.driver, "log");
        assert_eq!(blank.from, config.from);
    }
}
