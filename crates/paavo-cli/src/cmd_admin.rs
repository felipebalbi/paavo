//! `paavo-cli admin ...`.

use crate::cli::AdminOp;
use crate::client::Client;
use anyhow::Result;

/// Dispatch `paavo-cli admin <op>`.
pub async fn op(client: &Client, op: AdminOp) -> Result<()> {
    match op {
        AdminOp::Purge => {
            client.post_json::<()>("/admin/purge", None).await?;
            println!("purged");
            Ok(())
        }
    }
}
