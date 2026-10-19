//! Name a terminal session after the title its agent keeps for it, read from the agent's store.

use anyhow::{bail, Context};

use crate::session::civilizations::is_default_civ_name;
use crate::session::instance::Instance;

/// Why a session cannot import its agent's title, or `None` when it can.
pub fn skip_reason(inst: &Instance) -> Option<&'static str> {
    if crate::agents::get_agent(&inst.tool).is_none_or(|agent| agent.native_title.is_none()) {
        Some("This agent keeps no title to import")
    } else if inst.is_structured() {
        Some("Structured-view sessions are named by Smart Session Rename")
    } else if inst.is_sandboxed() {
        Some("Not available for sandboxed sessions")
    } else if inst.agent_session_id.is_none() {
        Some("The agent has not reported its session yet")
    } else {
        None
    }
}

/// The host store: the configured agent dir, else the agent's store variable, else its default
/// under the home dir, each read from the session's effective environment, which is what the
/// agent was launched with.
fn store_dir(inst: &Instance, native: crate::agents::NativeTitle) -> Option<std::path::PathBuf> {
    let environment = inst.resolved_host_environment();
    let home = crate::hooks::resolve_config_dir_override("HOME", &environment)
        .map(std::path::PathBuf::from)
        .or_else(dirs::home_dir)?;
    let declared = crate::session::config::repo_config::resolve_config_with_repo_or_warn(
        &inst.source_profile,
        std::path::Path::new(&inst.project_path),
    )
    .session
    .agent_config_dir_for(&inst.tool, &home);
    Some(store_dir_in(declared, native, &environment, &home))
}

fn store_dir_in(
    declared: Option<std::path::PathBuf>,
    native: crate::agents::NativeTitle,
    environment: &[String],
    home: &std::path::Path,
) -> std::path::PathBuf {
    declared
        .or_else(|| {
            crate::hooks::resolve_config_dir_override(native.store_env_var, environment)
                .map(Into::into)
        })
        .unwrap_or_else(|| home.join(native.default_store))
}

/// Set the session title to its agent's title. Without `force`, only a still generated name is
/// replaced. Returns whether the title changed.
pub fn import_agent_title(profile: &str, session_id: &str, force: bool) -> anyhow::Result<bool> {
    let storage = crate::session::storage::Storage::open_unwatched(profile)?;
    let inst = storage
        .load()?
        .into_iter()
        .find(|i| i.id == session_id)
        .context("Session not found")?;
    if let Some(reason) = skip_reason(&inst) {
        bail!(reason);
    }
    if !force && !is_default_civ_name(&inst.title) {
        bail!("Session already has a custom name");
    }
    let native = crate::agents::get_agent(&inst.tool)
        .and_then(|agent| agent.native_title)
        .context("This agent keeps no title to import")?;
    let agent_session_id = inst.agent_session_id.as_deref().unwrap_or_default();
    let title = store_dir(&inst, native)
        .and_then(|store| (native.read)(&store, agent_session_id))
        .context("The agent has no title for this session yet")?;
    crate::session::smart_rename::write_title(&storage, session_id, Some(&title), force, false)
}

/// Idle-edge hook: import in the background. A missing message retries on the next idle edge, so a
/// failed import never hands the session to smart rename.
pub fn spawn_auto_import(inst: &Instance) {
    let (profile, id) = (inst.source_profile.clone(), inst.id.clone());
    std::thread::spawn(move || {
        if let Err(error) = import_agent_title(&profile, &id, false) {
            tracing::debug!(target: "smart_rename", session = %id, "agent title import skipped: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_dir_follows_the_session_environment_over_ambient() {
        use std::path::{Path, PathBuf};
        let native = crate::agents::get_agent("codex")
            .unwrap()
            .native_title
            .unwrap();
        let env = ["CODEX_HOME=/profile/store".to_string()];
        let home = Path::new("/home/u");
        assert_eq!(
            store_dir_in(None, native, &env, home),
            PathBuf::from("/profile/store")
        );
        assert_eq!(
            store_dir_in(Some(PathBuf::from("/declared")), native, &env, home),
            PathBuf::from("/declared")
        );
    }

    #[test]
    fn skip_reason_follows_the_agent_capability() {
        let bound = |tool: &str| {
            let mut inst = Instance::new("Vikings", "/tmp/x");
            inst.tool = tool.into();
            inst.agent_session_id = Some("abcdef01-2345-6789-abcd-ef0123456789".into());
            inst
        };
        let mut unbound = bound("codex");
        unbound.agent_session_id = None;
        for (name, inst, importable) in [
            ("agent with a native title", bound("codex"), true),
            ("agent without one", bound("claude"), false),
            ("unknown tool", bound("no-such-agent"), false),
            ("session id not reported", unbound, false),
        ] {
            assert_eq!(skip_reason(&inst).is_none(), importable, "{name}");
        }
    }
}
