//! `agent-of-empires group` subcommands implementation

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::acp::client::{require_daemon, HttpClient, HttpError};
use crate::session::{GroupTree, Storage};

#[derive(Subcommand)]
pub enum GroupCommands {
    /// List all groups
    #[command(alias = "ls")]
    List(GroupListArgs),

    /// Create a new group
    Create(GroupCreateArgs),

    /// Delete a group
    Delete(GroupDeleteArgs),

    /// Move session to group
    Move(GroupMoveArgs),
}

#[derive(Args)]
pub struct GroupListArgs {
    /// Output as JSON
    #[arg(long)]
    pub(crate) json: bool,
}

impl GroupListArgs {
    pub(crate) fn json(&self) -> bool {
        self.json
    }
}

#[derive(Args)]
pub struct GroupCreateArgs {
    /// Group name
    name: String,

    /// Parent group for creating subgroups
    #[arg(long)]
    parent: Option<String>,
}

#[derive(Args)]
pub struct GroupDeleteArgs {
    /// Group name
    name: String,

    /// Force delete by moving sessions to default group
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
pub struct GroupMoveArgs {
    /// Session ID or title
    identifier: String,

    /// Target group
    group: String,
}

#[derive(Serialize)]
struct GroupInfo {
    name: String,
    path: String,
    session_count: usize,
    children: Vec<String>,
}

#[tracing::instrument(target = "cli.session", skip_all, fields(profile = %profile))]
pub async fn run(profile: &str, command: GroupCommands) -> Result<()> {
    match command {
        GroupCommands::List(args) => list_groups(profile, args).await,
        GroupCommands::Create(args) => create_group(profile, args).await,
        GroupCommands::Delete(args) => delete_group(profile, args).await,
        GroupCommands::Move(args) => move_session(profile, args).await,
    }
}

async fn list_groups(profile: &str, args: GroupListArgs) -> Result<()> {
    let storage = Storage::open_unwatched(profile)?;
    let (instances, groups) = storage.load_with_groups()?;

    let group_tree = GroupTree::new_with_groups(&instances, &groups);
    let session_count = |path: &str| instances.iter().filter(|i| i.group_path == path).count();

    if args.json {
        let group_list: Vec<GroupInfo> = group_tree
            .get_all_groups()
            .iter()
            .map(|g| GroupInfo {
                name: g.name.clone(),
                path: g.path.clone(),
                session_count: session_count(&g.path),
                children: g.children.iter().map(|c| c.name.clone()).collect(),
            })
            .collect();
        super::output::print_json(&group_list)?;
    } else {
        let all_groups = group_tree.get_all_groups();
        if all_groups.is_empty() {
            println!("No groups found.");
            println!("Create one with: aoe group create <name>");
            return Ok(());
        }

        println!("Groups:\n");
        for group in &all_groups {
            let session_count = session_count(&group.path);
            let indent = group.path.matches('/').count();
            println!(
                "{}• {} ({} sessions)",
                "  ".repeat(indent),
                group.name,
                session_count
            );
        }
        println!("\nTotal: {} groups", all_groups.len());
    }

    Ok(())
}

async fn create_group(profile: &str, args: GroupCreateArgs) -> Result<()> {
    let name = args.name.trim();
    let parent = args.parent.as_deref().map(str::trim);
    let client = HttpClient::new(require_daemon().await?)?;
    client
        .create_group(name, parent, profile)
        .await
        .map_err(group_error)?;

    match parent {
        Some(parent) => println!("✓ Created group: {parent}/{name}"),
        None => println!("✓ Created group: {name}"),
    }
    Ok(())
}

async fn delete_group(profile: &str, args: GroupDeleteArgs) -> Result<()> {
    let name = args.name.trim();
    let client = HttpClient::new(require_daemon().await?)?;
    let moved = client
        .delete_group(name, args.force, profile)
        .await
        .map_err(group_error)?;

    println!("✓ Deleted group: {name}");
    if moved > 0 {
        println!("  Moved {moved} sessions to default group");
    }
    Ok(())
}

/// Surface the Daemon's `message` for a rejected group change.
fn group_error(err: HttpError) -> anyhow::Error {
    if let HttpError::Server { body, .. } = &err {
        if let Some(message) = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v["message"].as_str().map(str::to_string))
        {
            return anyhow::anyhow!(message);
        }
    }
    err.into()
}

async fn move_session(profile: &str, args: GroupMoveArgs) -> Result<()> {
    let storage = Storage::open_unwatched(profile)?;
    let identifier = args.identifier.trim().to_string();
    let group = args.group.trim().to_string();

    let old_group = storage.update(|instances, groups| {
        let old = super::patch_instance(instances, &identifier, |inst| {
            let old = inst.group_path.clone();
            inst.group_path = group.clone();
            Ok(old)
        })?;

        if !group.is_empty() {
            let mut group_tree = GroupTree::new_with_groups(instances, groups);
            group_tree.create_group(&group);
            *groups = group_tree.get_all_groups();
        }
        Ok(old)
    })?;

    if old_group.is_empty() {
        println!("✓ Moved session to group: {}", group);
    } else {
        println!("✓ Moved session from '{}' to '{}'", old_group, group);
    }

    Ok(())
}
