//! Name a Codex terminal session after its first user message, read from its rollout file.

use anyhow::{bail, Context};

use crate::session::civilizations::is_default_civ_name;
use crate::session::instance::Instance;

/// Why a session cannot import a Codex title, or `None` when it can.
pub fn skip_reason(inst: &Instance) -> Option<&'static str> {
    if inst.tool != "codex" {
        Some("Session is not a Codex session")
    } else if inst.is_structured() {
        Some("Structured-view sessions are named by Smart Session Rename")
    } else if inst.is_sandboxed() {
        Some("Not available for sandboxed sessions")
    } else if inst.agent_session_id.is_none() {
        Some("Codex has not reported its session yet")
    } else {
        None
    }
}

/// The host Codex home: the configured agent dir, else `CODEX_HOME`, else `~/.codex`, each read
/// from the session's effective environment, which is what its Codex was launched with.
fn codex_home(inst: &Instance) -> Option<std::path::PathBuf> {
    let environment = inst.resolved_host_environment();
    let home = crate::hooks::resolve_config_dir_override("HOME", &environment)
        .map(std::path::PathBuf::from)
        .or_else(dirs::home_dir)?;
    let declared = crate::session::config::repo_config::resolve_config_with_repo_or_warn(
        &inst.source_profile,
        std::path::Path::new(&inst.project_path),
    )
    .session
    .agent_config_dir_for("codex", &home);
    Some(codex_home_in(declared, &environment, &home))
}

fn codex_home_in(
    declared: Option<std::path::PathBuf>,
    environment: &[String],
    home: &std::path::Path,
) -> std::path::PathBuf {
    declared
        .or_else(|| {
            crate::hooks::resolve_config_dir_override("CODEX_HOME", environment).map(Into::into)
        })
        .unwrap_or_else(|| home.join(".codex"))
}

/// Set the session title to its Codex title. Without `force`, only a still generated name is
/// replaced. Returns whether the title changed.
pub fn import_codex_title(profile: &str, session_id: &str, force: bool) -> anyhow::Result<bool> {
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
    let codex_session_id = inst.agent_session_id.as_deref().unwrap_or_default();
    let title = codex_home(&inst)
        .and_then(|home| crate::session::capture::codex_rollout_title(&home, codex_session_id))
        .context("Codex has no user message for this session yet")?;
    crate::session::smart_rename::write_title(&storage, session_id, Some(&title), force, false)
}

/// Idle-edge hook: import in the background. A missing message retries on the next idle edge, so a
/// failed import never hands the session to smart rename.
pub fn spawn_auto_import(inst: &Instance) {
    let (profile, id) = (inst.source_profile.clone(), inst.id.clone());
    std::thread::spawn(move || {
        if let Err(error) = import_codex_title(&profile, &id, false) {
            tracing::debug!(target: "smart_rename", session = %id, "codex title import skipped: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_home_follows_the_session_environment_over_ambient() {
        use std::path::{Path, PathBuf};
        let env = ["CODEX_HOME=/profile/store".to_string()];
        let home = Path::new("/home/u");
        let declared = Some(PathBuf::from("/declared"));
        assert_eq!(
            codex_home_in(None, &env, home),
            PathBuf::from("/profile/store")
        );
        assert_eq!(
            codex_home_in(declared, &env, home),
            PathBuf::from("/declared")
        );
    }

    #[test]
    fn skip_reason_covers_each_ineligible_session() {
        let mut codex = Instance::new("Vikings", "/tmp/x");
        codex.tool = "codex".into();
        codex.agent_session_id = Some("abcdef01-2345-6789-abcd-ef0123456789".into());
        assert_eq!(skip_reason(&codex), None);

        let mut claude = codex.clone();
        claude.tool = "claude".into();
        let mut unbound = codex.clone();
        unbound.agent_session_id = None;
        for (name, inst) in [("not codex", claude), ("no session id", unbound)] {
            assert!(skip_reason(&inst).is_some(), "{name}");
        }
    }
}
