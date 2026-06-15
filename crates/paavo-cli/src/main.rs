//! paavo-cli entry point.

use anyhow::{Context, Result};
use clap::Parser;

mod cli;
mod client;
mod cmd_admin;
mod cmd_boards;
mod cmd_jobs;
mod cmd_new;
mod cmd_run;
mod config;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let args = cli::Cli::parse();
    let host = config::resolve_host(args.host.as_deref())?;
    let client = client::Client::new(host);
    match args.cmd {
        cli::Cmd::Run {
            path,
            board_kind,
            instance,
            timeout,
            inactivity,
            priority,
        } => {
            cmd_run::run(
                &client,
                &path,
                board_kind.as_deref(),
                instance.as_deref(),
                timeout.as_deref(),
                inactivity.as_deref(),
                priority,
            )
            .await
        }
        cli::Cmd::New {
            name,
            board_kind,
            kind,
            into,
            templates_path,
        } => {
            let kind_str = match kind {
                cli::TestKindArg::Quick => "quick",
                cli::TestKindArg::Soak => "soak",
            }
            .to_string();
            let into = match into {
                Some(p) => p,
                None => std::env::current_dir()
                    .context("resolving current directory for default --into")?,
            };
            let code = cmd_new::run(cmd_new::NewArgs {
                crate_name: name,
                board_kind,
                kind: kind_str,
                into,
                templates_path,
            })?;
            std::process::exit(code);
        }
        cli::Cmd::Cancel { job_id } => cmd_jobs::cancel(&client, &job_id).await,
        cli::Cmd::Logs { job_id, follow } => cmd_jobs::logs(&client, &job_id, follow).await,
        cli::Cmd::Jobs { state, limit } => cmd_jobs::list(&client, state.as_deref(), limit).await,
        cli::Cmd::Boards => cmd_boards::list(&client).await,
        cli::Cmd::Board { op } => cmd_boards::op(&client, op).await,
        cli::Cmd::Admin { op } => cmd_admin::op(&client, op).await,
    }
}
