//! Basic usage example
//! 
//! Run with: cargo run --example basic_usage -- lin_api_YOUR_KEY

use herdr_linear::LinearClient;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = env::args()
        .nth(1)
        .or_else(|| env::var("LINEAR_API_KEY").ok())
        .ok_or("Please provide LINEAR_API_KEY as argument or environment variable")?;

    let client = LinearClient::new(&api_key)?;

    println!("🔐 Authenticating...");
    let viewer = client.get_viewer().await?;
    println!("✅ Authenticated as: {} ({})\n", viewer.name, viewer.email);

    println!("📋 Fetching teams...");
    let teams = client.get_teams(Some(10), None).await?;
    println!("✅ Found {} teams:\n", teams.total_count);
    for team in teams.nodes.iter().take(5) {
        println!("   • {} ({})", team.name, team.key);
    }

    if !teams.nodes.is_empty() {
        let first_team = &teams.nodes[0];
        println!("\n📌 Issues in team '{}':\n", first_team.name);

        match client.get_team_issues(&first_team.id, Some(5)).await {
            Ok(issues) => {
                println!("   Found {} issues:", issues.total_count);
                for issue in issues.nodes.iter().take(3) {
                    println!(
                        "     • {} [{}] - {}",
                        issue.identifier, issue.state.name, issue.title
                    );
                }
            }
            Err(e) => println!("   Error fetching issues: {}", e),
        }
    }

    println!("\n✨ Example completed!");
    Ok(())
}
