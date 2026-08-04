//! Example: Working with issues
//! 
//! Run with: cargo run --example issue_operations -- lin_api_YOUR_KEY TEAM_ID

use herdr_linear::{LinearClient, Error};
use serde_json::json;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    
    let api_key = args
        .get(1)
        .cloned()
        .or_else(|| env::var("LINEAR_API_KEY").ok())
        .ok_or("Please provide LINEAR_API_KEY as argument or env var")?;

    let team_id = args
        .get(2)
        .cloned()
        .ok_or("Please provide TEAM_ID as second argument")?;

    let client = LinearClient::new(&api_key)?;

    println!("🔐 Authenticating...");
    let viewer = client.get_viewer().await?;
    println!("✅ Authenticated as: {}\n", viewer.name);

    // Example 1: List issues
    println!("📋 Fetching issues...");
    match client.get_team_issues(&team_id, Some(5)).await {
        Ok(issues) => {
            println!("✅ Found {} issues:\n", issues.total_count);
            for (i, issue) in issues.nodes.iter().take(3).enumerate() {
                println!(
                    "  {}. {} [{}]\n     Title: {}\n     Assigned to: {}\n",
                    i + 1,
                    issue.identifier,
                    issue.state.name,
                    issue.title,
                    issue
                        .assignee
                        .as_ref()
                        .map(|a| a.name.as_str())
                        .unwrap_or("Unassigned")
                );
            }

            // Example 2: Get details of first issue
            if let Some(first_issue) = issues.nodes.first() {
                println!("📍 Getting details for {}...", first_issue.identifier);
                match client.get_issue(&first_issue.id).await {
                    Ok(issue) => {
                        println!("✅ Issue details:");
                        println!("   ID: {}", issue.id);
                        println!("   Identifier: {}", issue.identifier);
                        println!("   Title: {}", issue.title);
                        println!("   Description: {}", 
                            issue.description.as_ref().unwrap_or(&"None".to_string()));
                        println!("   Priority: {}", issue.priority);
                        println!("   Estimate: {:?} pts", issue.estimate);
                        println!("   State: {}", issue.state.name);
                        if let Some(assignee) = issue.assignee {
                            println!("   Assigned to: {}", assignee.name);
                        }
                        println!("   Created: {}", issue.created_at);
                        println!("   Updated: {}", issue.updated_at);
                        if !issue.labels.is_empty() {
                            print!("   Labels: ");
                            for label in &issue.labels {
                                print!("{} ", label.name);
                            }
                            println!();
                        }

                        // Example 3: Add a comment
                        let comment_body = "Automated comment from herdr-linear example";
                        println!("\n💬 Adding comment...");
                        match client.add_comment(&first_issue.id, &comment_body).await {
                            Ok(comment) => {
                                println!("✅ Comment added:");
                                println!("   By: {}", comment.user.name);
                                println!("   Body: {}", comment.body);
                            }
                            Err(e) => println!("❌ Error adding comment: {}", e),
                        }
                    }
                    Err(e) => println!("❌ Error fetching issue details: {}", e),
                }
            }
        }
        Err(e) => {
            match e {
                Error::AuthenticationFailed(msg) => println!("❌ Auth failed: {}", msg),
                _ => println!("❌ Error: {}", e),
            }
        }
    }

    println!("\n✨ Example completed!");
    Ok(())
}
