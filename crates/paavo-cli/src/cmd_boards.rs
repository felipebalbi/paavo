//! `paavo-cli boards | board ...`.

use crate::cli::BoardOp;
use crate::client::Client;
use anyhow::{anyhow, Result};
use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};
use serde_json::Value;

/// `paavo-cli boards`.
pub async fn list(client: &Client) -> Result<()> {
    let rows: Vec<Value> = client.get_json("/boards").await?;
    for r in rows {
        println!(
            "{id:18} {kind:12} {health:13} {target}",
            id = r["id"].as_str().unwrap_or(""),
            kind = r["kind"].as_str().unwrap_or(""),
            health = r["health"].as_str().unwrap_or(""),
            target = r["target_name"].as_str().unwrap_or(""),
        );
    }
    Ok(())
}

/// `paavo-cli board ...`.
pub async fn op(client: &Client, op: BoardOp) -> Result<()> {
    match op {
        BoardOp::Add {
            kind,
            instance,
            probe,
            chip,
            target,
            wiring_profile,
        } => {
            let probe_selector = ProbeSelector::parse(&probe).map_err(|e| {
                anyhow!(
                    "invalid --probe {probe:?}: {e}\n\n\
                     Paste a probe-rs selector (`1fc9:0143:SERIAL`) or a full \
                     `probe-rs list` line."
                )
            })?;
            let spec = BoardSpec {
                id: instance,
                kind,
                probe_selector,
                chip_name: chip,
                target_name: target,
                wiring_profile: Some(wiring_profile),
                health: BoardHealth::Healthy,
            };
            client.add_board(&spec).await?;
            println!("added: {}", spec.id);
            Ok(())
        }
        BoardOp::Quarantine { id, reason } => {
            #[derive(serde::Serialize)]
            struct Body<'a> {
                reason: &'a str,
            }
            client
                .post_json(
                    &format!("/boards/{id}/quarantine"),
                    Some(&Body { reason: &reason }),
                )
                .await?;
            println!("quarantined: {id}");
            Ok(())
        }
        BoardOp::Unquarantine { id } => {
            client
                .post_json::<()>(&format!("/boards/{id}/unquarantine"), None)
                .await?;
            println!("unquarantined: {id}");
            Ok(())
        }
        BoardOp::Remove { id } => {
            client.delete_json(&format!("/boards/{id}")).await?;
            println!("removed: {id}");
            Ok(())
        }
    }
}
