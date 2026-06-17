//! `paavo-cli admin ...`.

use crate::cli::AdminOp;
use crate::client::Client;
use anyhow::Result;
use std::io::{self, Write};

/// Dispatch `paavo-cli admin <op>`.
pub async fn op(client: &Client, op: AdminOp) -> Result<()> {
    match op {
        AdminOp::Purge { boards, yes } => {
            if boards && !yes && !confirm_board_purge()? {
                println!("purge aborted");
                return Ok(());
            }
            // The board wipe is opt-in via a query param so the default
            // path stays byte-for-byte the original request.
            let path = if boards {
                "/admin/purge?boards=true"
            } else {
                "/admin/purge"
            };
            client.post_json::<()>(path, None).await?;
            println!(
                "{}",
                if boards {
                    "purged (including boards)"
                } else {
                    "purged"
                }
            );
            Ok(())
        }
    }
}

/// Prompt the operator before the destructive board wipe. Reads one
/// line from stdin and returns `true` only for `y`/`yes`
/// (case-insensitive, trimmed). Empty input, EOF, or anything else
/// returns `false` — the prompt defaults to "no".
fn confirm_board_purge() -> Result<bool> {
    println!(
        "WARNING: --boards permanently deletes ALL boards from the inventory, \
         on top of wiping all jobs, logs, build cache, and on-disk artifacts."
    );
    print!("Purge ALL boards too? [y/N]: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let ans = line.trim().to_ascii_lowercase();
    Ok(ans == "y" || ans == "yes")
}
