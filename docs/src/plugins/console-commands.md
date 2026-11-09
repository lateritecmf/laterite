# Console Commands

A module adds a command to the application. `lat <name>` runs it from the
project, after the application has booted.

## Register a command

```rust
use std::sync::Arc;
use laterite_core::command::{Command, CommandCx, CommandError, CommandReg};
use laterite_core::t;

struct Greet;

#[async_trait::async_trait]
impl Command for Greet {
    async fn run(&self, cx: &CommandCx<'_>, args: &[String]) -> Result<(), CommandError> {
        let name = args.first().ok_or("usage: greet <name>")?;
        println!("Hello {name}, from {}", cx.storage_dir());
        Ok(())
    }
}

impl Module for AcmeBlog {
    fn register(&self, registry: &mut Registry) {
        registry.add(CommandReg::new("greet", t!("Greet someone"), Arc::new(Greet)));
    }
}
```

```bash
lat greet Ada
lat help-app
```

`CommandReg::new` | Description
--- | ---
`name` | The word that runs it: lowercase letters, digits and hyphens.
`about` | One line in `lat help-app`.
`command` | What runs.

`CommandCx` | Gives
--- | ---
`db()` | The application's database, migrated.
`registry()` | Every module's registrations.
`storage_dir()` | `backend.storage`.

## Parse arguments

```rust
use clap::Parser;

#[derive(Parser)]
struct Args {
    name: String,
    #[arg(long)]
    loud: bool,
}

async fn run(&self, cx: &CommandCx<'_>, args: &[String]) -> Result<(), CommandError> {
    let args = Args::try_parse_from(std::iter::once("greet".to_string()).chain(args.iter().cloned()))?;
    println!("{}", if args.loud { args.name.to_uppercase() } else { args.name });
    Ok(())
}
```

Case | Result
--- | ---
`run` returns an error | The message is printed and the binary exits non-zero.
A name the binary answers itself (`serve`, `admin`, `routes`, `seed`, `build`, `publish`, `queue`, `schedule`, `help`) | The start aborts.
Two modules register one name | The start aborts.
An unknown name | The error lists every command.

The application's `main` must call `Bootstrap::run`, as `lat new` writes it.
