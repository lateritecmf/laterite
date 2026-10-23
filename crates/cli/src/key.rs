//! `lat key`: the application key everything encrypts and signs under.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use laterite_core::MasterKey;

use crate::project::Project;

#[derive(Subcommand)]
pub enum KeyCommand {
    /// Generate an application key and write it to config/local.toml as app.key.
    Generate(GenerateArgs),
    /// Show the id of the key in use and of the keys kept for reading.
    Show,
}

#[derive(Args)]
pub struct GenerateArgs {
    /// Print the key and write nothing.
    #[arg(long)]
    print: bool,
    /// Keep the key in use as a previous key, so what it sealed still opens
    /// until `lat crypt rotate` re-seals it.
    #[arg(long)]
    rotate: bool,
}

pub async fn run(command: KeyCommand, database_url: Option<String>) -> Result<()> {
    match command {
        KeyCommand::Generate(args) => generate(args, database_url).await,
        KeyCommand::Show => show(),
    }
}

fn show() -> Result<()> {
    let project = Project::locate()?;
    let config: laterite_admin::AppConfig = project.load()?;
    match config.app.key.as_deref() {
        Some(key) => {
            let parsed = MasterKey::parse(key).context("app.key does not parse")?;
            println!("app.key: id {}", parsed.id());
        }
        None => println!(
            "app.key: unset (a key derived from backend.secret or the stored key is in use)"
        ),
    }
    for previous in &config.app.previous_keys {
        match MasterKey::parse(previous) {
            Ok(parsed) => println!("previous: id {}", parsed.id()),
            Err(e) => println!("previous: {e}"),
        }
    }
    Ok(())
}

async fn generate(args: GenerateArgs, database_url: Option<String>) -> Result<()> {
    let project = Project::locate().ok();
    let config: Option<laterite_admin::AppConfig> = match &project {
        Some(p) => Some(p.load()?),
        None => None,
    };
    let current = config
        .as_ref()
        .and_then(|c| c.app.key.as_deref())
        .map(|k| MasterKey::parse(k).context("the current app.key does not parse"))
        .transpose()?;
    let next_id = match &current {
        Some(key) => {
            let mut id = key.id().wrapping_add(1);
            if id == 0 {
                id = 1;
            }
            id
        }
        None => 1,
    };
    let key = MasterKey::generate(next_id);
    let exported = key.export();
    let Some(project) = project.filter(|_| !args.print) else {
        println!("{exported}");
        println!("Set it as app.key in config/local.toml, or as the APP__KEY environment variable under the app's prefix.");
        return Ok(());
    };
    let path = project.config_dir.join("local.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut previous: Vec<String> = config
        .as_ref()
        .map(|c| c.app.previous_keys.clone())
        .unwrap_or_default();
    if args.rotate {
        match &current {
            Some(old) => previous.push(old.export()),
            // No app.key yet: the key in use derives from backend.secret or the
            // stored key, so that derivation is kept as the previous key.
            None => {
                let config = config.as_ref().context("no application configuration")?;
                let db = crate::connect(database_url).await?;
                let legacy =
                    laterite_admin::legacy_key_material(&db, config.backend.secret.as_deref())
                        .await
                        .context("reading the stored key")?;
                previous.push(MasterKey::derived(0, &legacy).export());
            }
        }
    }
    let updated = write_app_keys(&text, &exported, args.rotate.then_some(&previous));
    std::fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))?;
    println!("Wrote app.key (id {next_id}) to {}.", path.display());
    if args.rotate {
        println!("The old key is kept under app.previous_keys; run `lat crypt rotate` to re-seal stored values, then remove it.");
    } else if current.is_some() {
        println!("The old key was replaced. Values sealed under it no longer open; pass --rotate to keep it readable.");
    }
    Ok(())
}

/// `text` with `key = "..."` (and `previous_keys`, when given) set in its
/// `[app]` section, which is added at the end when absent. Other lines stay.
fn write_app_keys(text: &str, key: &str, previous: Option<&Vec<String>>) -> String {
    let key_line = format!("key = \"{key}\"");
    let previous_line = previous.map(|list| {
        let items: Vec<String> = list.iter().map(|k| format!("\"{k}\"")).collect();
        format!("previous_keys = [{}]", items.join(", "))
    });
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let section = lines.iter().position(|l| l.trim() == "[app]");
    let Some(start) = section else {
        if !text.is_empty() && !text.ends_with('\n') {
            lines.push(String::new());
        }
        lines.push(String::new());
        lines.push("[app]".to_string());
        lines.push(key_line);
        if let Some(line) = previous_line {
            lines.push(line);
        }
        return lines.join("\n") + "\n";
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map(|i| start + 1 + i)
        .unwrap_or(lines.len());
    let mut key_at = None;
    let mut wrote_previous = previous_line.is_none();
    for (index, line) in lines.iter_mut().enumerate().take(end).skip(start + 1) {
        let bare = line.trim_start();
        if bare.starts_with("key ") || bare.starts_with("key=") {
            *line = key_line.clone();
            key_at = Some(index);
        } else if let Some(p) = &previous_line {
            if bare.starts_with("previous_keys") {
                *line = p.clone();
                wrote_previous = true;
            }
        }
    }
    let key_at = match key_at {
        Some(at) => at,
        None => {
            lines.insert(start + 1, key_line);
            start + 1
        }
    };
    if !wrote_previous {
        if let Some(p) = previous_line {
            lines.insert(key_at + 1, p);
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_lands_in_the_app_section_whether_or_not_one_exists() {
        let fresh = write_app_keys("[database]\nurl = \"sqlite://x\"\n", "1:AAAA", None);
        assert_eq!(
            fresh,
            "[database]\nurl = \"sqlite://x\"\n\n[app]\nkey = \"1:AAAA\"\n"
        );
        let existing = "[app]\nname = \"Acme\"\nkey = \"1:OLD\"\n\n[database]\nurl = \"x\"\n";
        let replaced = write_app_keys(existing, "2:NEW", Some(&vec!["1:OLD".to_string()]));
        assert_eq!(
            replaced,
            "[app]\nname = \"Acme\"\nkey = \"2:NEW\"\nprevious_keys = [\"1:OLD\"]\n\n[database]\nurl = \"x\"\n"
        );
        let appended = write_app_keys("[app]\nname = \"Acme\"\n", "1:K", None);
        assert_eq!(appended, "[app]\nkey = \"1:K\"\nname = \"Acme\"\n");
    }
}
