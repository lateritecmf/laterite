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
    /// The sender name messages carry; blank keeps the config's.
    pub from_name: String,
    #[serde(default)]
    /// The sender address messages carry; blank keeps the config's.
    pub from_address: String,
    /// `smtp`, `log`, `stub` or a registered driver; blank keeps the config.
    #[serde(default)]
    pub driver: String,
    #[serde(default)]
    /// The SMTP server's host name; blank keeps the config's.
    pub smtp_host: String,
    #[serde(default)]
    /// The SMTP server's port, as text; blank keeps the config's.
    pub smtp_port: String,
    #[serde(default)]
    /// `starttls`, `tls` or `none`; blank keeps the config's.
    pub smtp_encryption: String,
    #[serde(default)]
    /// The SMTP user name; blank keeps the config's.
    pub smtp_username: String,
    /// Sealed by the `sensitive` field type.
    #[serde(default)]
    pub smtp_password: String,
    /// A development override: every message goes here while set.
    #[serde(default)]
    pub to_override: String,
    /// The values of the drivers' own fields, each under
    /// `opt_<driver>_<key>`; a secret one is sealed.
    #[serde(flatten)]
    pub options: std::collections::BTreeMap<String, serde_json::Value>,
}

impl SettingsModel for MailSetting {
    const CODE: &'static str = "laterite.mail";
}

/// The name a driver's own field is saved and posted under.
pub fn option_field(driver: &str, key: &str) -> String {
    format!("opt_{driver}_{key}")
}

/// The values the selected driver's own fields hold, by the driver's keys,
/// with each secret opened. `driver` is the one in force.
pub fn driver_options(
    driver: &str,
    setting: &MailSetting,
    drivers: &[laterite_core::mail::MailDriverReg],
    crypt: &Crypt,
) -> laterite_core::mail::DriverOptions {
    let mut out = laterite_core::mail::DriverOptions::new();
    let Some(reg) = drivers.iter().find(|r| r.name() == driver) else {
        return out;
    };
    for field in reg.fields() {
        let name = option_field(driver, field.key);
        let Some(stored) = setting
            .options
            .get(&name)
            .and_then(|v| v.as_str())
            .map(str::trim)
        else {
            continue;
        };
        if stored.is_empty() || stored == MASK {
            continue;
        }
        if field.secret {
            match crypt.decrypt(FIELD_PURPOSE, name.as_bytes(), stored) {
                Ok(plain) => {
                    out.insert(
                        field.key.to_string(),
                        String::from_utf8_lossy(&plain).to_string(),
                    );
                }
                Err(_) => tracing::warn!(
                    field = %name,
                    "a saved mail driver secret does not open under the current keys"
                ),
            }
        } else {
            out.insert(field.key.to_string(), stored.to_string());
        }
    }
    out
}

/// The settings screen under System, offering the built-in drivers.
pub fn settings_item() -> SettingsItem {
    settings_item_for(&laterite_core::mail::drivers(Vec::new()))
}

/// The settings screen under System, offering `drivers`: each is a choice of
/// "Send through", and its own fields show while it is chosen.
pub fn settings_item_for(drivers: &[laterite_core::mail::MailDriverReg]) -> SettingsItem {
    let mut choices: Vec<(String, String)> = vec![
        (String::new(), "Configured driver".to_string()),
        ("smtp".to_string(), "SMTP".to_string()),
        ("log".to_string(), "Mail log (nothing leaves)".to_string()),
        ("stub".to_string(), "Nowhere (tests)".to_string()),
    ];
    let mut driver_fields: Vec<FormField> = Vec::new();
    for reg in drivers {
        if !choices.iter().any(|(value, _)| value == reg.name()) {
            let label = reg
                .display()
                .map(|text| text.source().to_string())
                .unwrap_or_else(|| reg.name().to_string());
            choices.push((reg.name().to_string(), label));
        }
        for field in reg.fields() {
            let name = option_field(reg.name(), field.key);
            let mut form_field = if field.secret {
                FormField::of(&name, field.label.clone(), "sensitive")
            } else {
                FormField::text(&name, field.label.clone())
            };
            if let Some(help) = &field.help {
                form_field = form_field.help(help.clone());
            }
            driver_fields.push(form_field.trigger(crate::form::Trigger::new(
                "show",
                "driver",
                format!("value[{}]", reg.name()),
            )));
        }
    }
    let smtp_only = |field: FormField| {
        field.trigger(crate::form::Trigger::new("show", "driver", "value[][smtp]"))
    };
    let mut fields = vec![
        FormField::text("from_name", t!("Sender name"))
            .help(t!("Shown as the sender; blank keeps the configured name.")),
        FormField::text("from_address", t!("Sender address"))
            .help(t!("Blank keeps the configured address.")),
        FormField::select(
            "driver",
            t!("Send through"),
            choices
                .iter()
                .map(|(value, label)| (value.as_str(), label.as_str()))
                .collect(),
        )
        .help(t!("Each choice shows the fields it needs.")),
        smtp_only(FormField::text("smtp_host", t!("SMTP host"))),
        smtp_only(
            FormField::text("smtp_port", t!("SMTP port"))
                .help(t!("587 for STARTTLS, 465 for TLS, 25 or 1025 for none.")),
        ),
        smtp_only(FormField::select(
            "smtp_encryption",
            t!("Encryption"),
            vec![
                ("", "Configured"),
                ("starttls", "STARTTLS"),
                ("tls", "TLS"),
                ("none", "None"),
            ],
        )),
        smtp_only(FormField::text("smtp_username", t!("SMTP username"))),
        smtp_only(FormField::of(
            "smtp_password",
            t!("SMTP password"),
            "sensitive",
        )),
    ];
    fields.extend(driver_fields);
    fields.push(
        FormField::text("to_override", t!("Send everything to")).help(t!(
            "While set, every message goes to this address instead of its recipients. For development."
        )),
    );
    SettingsItem::new(MailSetting::CODE, t!("Mail Configuration"), fields)
    .description(t!("The sender, the transport and a test message."))
    .hint(t!("Blank fields keep the values from the configuration file. Save, then send a test message to yourself."))
    .category(super::category::MAIL)
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
    #[test]
    fn a_driver_brings_its_own_fields_and_reads_them_back() {
        use laterite_core::mail::{MailDriverField, MailDriverReg};
        let drivers =
            laterite_core::mail::drivers(vec![MailDriverReg::with_options("acme", |_, _, _| {
                Err(laterite_core::CoreError::Config("not built here".into()))
            })
            .label(t!("Acme Mail"))
            .field(MailDriverField::new("domain", t!("Domain")))
            .field(MailDriverField::new("secret", t!("API key")).secret())]);
        let item = settings_item_for(&drivers);
        let names: Vec<&str> = item.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"opt_acme_domain"));
        assert!(names.contains(&"opt_acme_secret"));
        // The built-in sendmail driver, where the build has it, asks for its
        // path the same way.
        let sendmail = drivers.iter().any(|d| d.name() == "sendmail");
        assert_eq!(names.contains(&"opt_sendmail_path"), sendmail);
        // The sender and the override stay first and last.
        assert_eq!(names.first(), Some(&"from_name"));
        assert_eq!(names.last(), Some(&"to_override"));

        let crypt = Crypt::ephemeral();
        let sealed = crypt
            .encrypt(FIELD_PURPOSE, b"opt_acme_secret", b"key-123")
            .unwrap();
        let setting: MailSetting = serde_json::from_value(serde_json::json!({
            "driver": "acme",
            "opt_acme_domain": " mail.acme.com ",
            "opt_acme_secret": sealed,
            "opt_sendmail_path": "/usr/sbin/sendmail",
        }))
        .unwrap();
        assert_eq!(setting.driver, "acme");
        let options = driver_options("acme", &setting, &drivers, &crypt);
        assert_eq!(
            options.get("domain").map(String::as_str),
            Some("mail.acme.com")
        );
        assert_eq!(options.get("secret").map(String::as_str), Some("key-123"));
        // Only the selected driver's values are handed over.
        assert_eq!(options.len(), 2);
        if sendmail {
            let path = driver_options("sendmail", &setting, &drivers, &crypt);
            assert_eq!(
                path.get("path").map(String::as_str),
                Some("/usr/sbin/sendmail")
            );
        }
        assert!(driver_options("log", &setting, &drivers, &crypt).is_empty());
    }
}
