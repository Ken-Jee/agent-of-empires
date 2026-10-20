//! `agent-of-empires agents` command implementation

use anyhow::Result;

#[tracing::instrument(target = "cli.agents", skip_all)]
pub fn run() -> Result<()> {
    let available = crate::tmux::AvailableTools::detect();
    let available_list = available.available_list();
    let colorize = crate::cli::color_enabled();
    let installed_mark = if colorize {
        "\x1b[32m✓\x1b[0m"
    } else {
        "✓"
    };
    let missing_mark = if colorize {
        "\x1b[31m✗\x1b[0m"
    } else {
        "✗"
    };

    println!("Supported AI coding agents:\n");

    for agent in crate::agents::AGENTS {
        let installed = available_list.iter().any(|s| s == agent.name);
        if installed {
            println!("  {installed_mark} {:<12} installed", agent.name);
        } else {
            println!(
                "  {missing_mark} {:<12} not installed -- {}",
                agent.name, agent.install_hint
            );
        }
        if let Some(notice) = agent.lifecycle_notice() {
            println!("{}", crate::cli::lifecycle_notice_line("      ", &notice));
        }
    }
    let plugin_agents: Vec<_> = crate::plugin::registry()
        .active()
        .flat_map(|plugin| {
            plugin
                .manifest
                .agent_integrations
                .iter()
                .filter(|integration| {
                    integration.command.is_some()
                        && crate::agents::get_agent(&integration.tool).is_none()
                })
                .map(move |integration| (plugin.id().to_string(), integration.tool.clone()))
        })
        .collect();
    for (plugin, name) in &plugin_agents {
        if available_list.iter().any(|installed| installed == name) {
            println!("  {installed_mark} {name:<12} installed (plugin: {plugin})");
        } else {
            println!("  {missing_mark} {name:<12} runtime not installed (plugin: {plugin})");
        }
    }

    let plugin_installed = plugin_agents
        .iter()
        .filter(|(_, name)| available_list.iter().any(|installed| installed == name))
        .count();
    let installed_count = plugin_installed
        + crate::agents::AGENTS
            .iter()
            .filter(|a| available_list.iter().any(|s| s == a.name))
            .count();
    let total_count = crate::agents::AGENTS.len() + plugin_agents.len();

    println!("\n{}/{} agents installed.", installed_count, total_count);

    if installed_count == 0 {
        println!("\nInstall at least one agent to get started.");
        println!("Recommended: npm install -g @anthropic-ai/claude-code");
    }

    Ok(())
}
