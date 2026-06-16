//! paavo-web entry point.
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "paavo-web", version)]
struct Args {
    /// Path to paavo.toml.
    #[arg(long, env = "PAAVO_CONFIG", default_value = "/etc/paavo/paavo.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let cfg = paavo_web::config::RootConfig::load(&args.config)?;
    let sqlite_path = cfg.server.state_dir.join("paavo.sqlite");
    let db = paavo_web::db::WebDb::open(&sqlite_path)?;
    // paavod_url is parsed at startup so a malformed value fails
    // here, not on the first SSE proxy request.
    let paavod = paavo_web::proxy::PaavodClient::new(&cfg.web.paavod_url)?;
    let state = paavo_web::proxy::AppState { db, paavod };
    let listener = tokio::net::TcpListener::bind(&cfg.web.bind).await?;
    tracing::info!(
        bind = %cfg.web.bind,
        paavod = %cfg.web.paavod_url,
        "paavo-web listening"
    );
    axum::serve(listener, paavo_web::app::build_router(state)).await?;
    Ok(())
}
