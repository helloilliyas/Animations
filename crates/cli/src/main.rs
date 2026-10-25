//! `cse`: CSE Trader from the command line.
//!
//! Every command prints JSON (pretty, or one line with `--compact`) so
//! scripts and AI agents can use it directly; errors go to stderr as
//! `{"error": "..."}` with exit code 1. It drives the same service as the
//! phone app (`cse_service::AppService`), with its own database on this
//! computer.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use cse_cli::{run, Cli};
use cse_service::AppService;

fn default_db() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("cse-trader").join("cse-trader.sqlite")
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let db = cli.db.clone().unwrap_or_else(default_db);
    if let Some(dir) = db.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let out = match AppService::open(&db) {
        Ok(svc) => run(&svc, &cli).await,
        Err(e) => Err(e.to_string()),
    };
    match out {
        Ok(v) => {
            let text = if cli.compact {
                serde_json::to_string(&v)
            } else {
                serde_json::to_string_pretty(&v)
            };
            println!("{}", text.unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", serde_json::json!({ "error": e }));
            ExitCode::FAILURE
        }
    }
}
