// SPDX-License-Identifier: AGPL-3.0-or-later
//! Explicit operator-only historical identity reconciliation; dry-run by default.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|v| v == "--help") {
        println!(
            "github-backfill <old-installation-id> [--evidence <trusted-response.json>] [--apply]\nUses DATABASE_URL. Defaults to read-only dry-run (transaction rolled back).\nWithout --evidence, requires GITHUB_APP_ID, GITHUB_APP_NAME and GITHUB_APP_PRIVATE_KEY_PATH.\nEvidence must be an authenticated historical GitHub installation response proving the exact old installation -> stable account ID; login-only records are insufficient."
        );
        return Ok(());
    }
    let installation_id: i64 = args[0].parse()?;
    let mut evidence = None;
    let mut apply = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--apply" => apply = true,
            "--evidence" => {
                index += 1;
                evidence = Some(args.get(index).ok_or("--evidence requires a path")?);
            }
            _ => return Err("Unknown argument; run --help".into()),
        }
        index += 1;
    }
    let details: trakkt_github::GitHubInstallationDetails = if let Some(path) = evidence {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        let app_id: u64 = std::env::var("GITHUB_APP_ID")?.parse()?;
        let key = std::fs::read(std::env::var("GITHUB_APP_PRIVATE_KEY_PATH")?)?;
        let name = std::env::var("GITHUB_APP_NAME")?;
        let client = trakkt_github::GitHubClient::new(app_id, &key, &name)?;
        client
            .get_installation_details(u64::try_from(installation_id)?)
            .await?
    };
    let db = trakkt_core::DbPool::connect(&std::env::var("DATABASE_URL")?).await?;
    trakkt_github::authorization::reconcile_legacy_identity(&db, installation_id, &details, apply)
        .await?;
    println!(
        "Historical identity {} for installation {} (account {}); connection status, links and tokens unchanged.",
        if apply {
            "recorded"
        } else {
            "validated (dry-run)"
        },
        installation_id,
        details.account.id
    );
    Ok(())
}
