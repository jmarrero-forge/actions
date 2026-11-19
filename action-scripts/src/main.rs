mod share;
mod tailscale;

use anyhow::{Context, Result, bail};
use std::{env, path::Path};

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("missing environment variable {name}"))
}

fn main() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    match args.as_slice() {
        [command, settings] if command == "tailscale-setup" => {
            tailscale::setup(Path::new(settings))
        }
        [command] if command == "tailscale-check" => tailscale::check(),
        [command] if command == "sandbox-share" => share::run(),
        [command] if command == "sandbox-share-check" => share::check(),
        _ => bail!(
            "usage: action-scripts tailscale-setup SETTINGS.json | tailscale-check | sandbox-share | sandbox-share-check"
        ),
    }
}
