//! Group registry mutations: `POST` and `DELETE /api/groups`.

use std::sync::Arc;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::{api_error, cityhall_block, read_only_block, validate_display_label};
use super::{validate_profile_name, AppState};
use crate::session::{GroupTree, Storage};

#[derive(Deserialize)]
pub struct CreateGroupBody {
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Deserialize)]
pub struct DeleteGroupQuery {
    pub path: String,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum GroupError {
    #[error("Group already exists: {0}")]
    AlreadyExists(String),
    #[error("Group not found: {0}")]
    NotFound(String),
    #[error(
        "Group '{path}' contains {count} sessions. Use --force to move them to default group."
    )]
    NotEmpty { path: String, count: usize },
}

fn in_group(group_path: &str, group: &str) -> bool {
    group_path == group
        || group_path
            .strip_prefix(group)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A profile the daemon manages: a valid name with an existing store.
fn resolve_profile(state: &AppState, requested: Option<String>) -> Result<String, Response> {
    let profile = requested.unwrap_or_else(|| state.profile.clone());
    validate_profile_name(&profile)
        .map_err(|msg| api_error(StatusCode::BAD_REQUEST, "invalid_profile", msg))?;
    let known = crate::session::list_profiles().is_ok_and(|all| all.contains(&profile));
    if !known {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "profile_not_found",
            format!("Profile not found: {profile}"),
        ));
    }
    Ok(profile)
}

fn group_error_response(err: anyhow::Error, label: &str) -> Response {
    match err.downcast::<GroupError>() {
        Ok(e) => {
            let (status, code) = match &e {
                GroupError::AlreadyExists(_) => (StatusCode::CONFLICT, "group_already_exists"),
                GroupError::NotFound(_) => (StatusCode::NOT_FOUND, "group_not_found"),
                GroupError::NotEmpty { .. } => (StatusCode::CONFLICT, "group_not_empty"),
            };
            api_error(status, code, e.to_string())
        }
        Err(e) => {
            tracing::error!(target: "http.api.groups", "Failed to {label} group: {e}");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "persist_failed",
                "Failed to persist group change",
            )
        }
    }
}

/// `POST /api/groups`. Registers a group, nested under `parent` when given.
pub async fn create_group(
    State(state): State<Arc<AppState>>,
    body: Result<Json<CreateGroupBody>, JsonRejection>,
) -> impl IntoResponse {
    if let Some(resp) = cityhall_block(&state) {
        return resp;
    }
    if let Some(resp) = read_only_block(&state) {
        return resp;
    }
    let Json(body) = match body {
        Ok(b) => b,
        Err(rej) => return rej.into_response(),
    };
    let name = body.name.trim();
    let path = match body.parent.as_deref().map(str::trim) {
        Some(parent) if !parent.is_empty() => format!("{parent}/{name}"),
        _ => name.to_string(),
    };
    if name.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_group_path",
            "Group name cannot be empty",
        );
    }
    if let Err(msg) = validate_display_label(&path, "group") {
        return api_error(StatusCode::BAD_REQUEST, "invalid_group_path", msg);
    }
    let profile = match resolve_profile(&state, body.profile) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let file_watch = state.file_watch.clone();
    let persist_path = path.clone();
    let result = tokio::task::spawn_blocking(move || {
        Storage::new(&profile, file_watch)?.update(|instances, groups| {
            let mut tree = GroupTree::new_with_groups(instances, groups);
            if tree.group_exists(&persist_path) {
                return Err(GroupError::AlreadyExists(persist_path).into());
            }
            tree.create_group(&persist_path);
            *groups = tree.get_all_groups();
            Ok(())
        })
    })
    .await;
    match result {
        Ok(Ok(())) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "path": path })),
        )
            .into_response(),
        Ok(Err(e)) => group_error_response(e, "create"),
        Err(e) => group_error_response(e.into(), "create"),
    }
}

/// `DELETE /api/groups?path=&force=&profile=`. Removes a group and its
/// subtree. Sessions inside block the delete unless `force` ungroups them.
pub async fn delete_group(
    State(state): State<Arc<AppState>>,
    query: Result<Query<DeleteGroupQuery>, QueryRejection>,
) -> impl IntoResponse {
    if let Some(resp) = cityhall_block(&state) {
        return resp;
    }
    if let Some(resp) = read_only_block(&state) {
        return resp;
    }
    let Query(query) = match query {
        Ok(q) => q,
        Err(rej) => return rej.into_response(),
    };
    let path = query.path.trim().to_string();
    let force = query.force;
    let profile = match resolve_profile(&state, query.profile) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let file_watch = state.file_watch.clone();
    let persist_path = path.clone();
    let persist_profile = profile.clone();
    let result = tokio::task::spawn_blocking(move || {
        Storage::new(&persist_profile, file_watch)?.update(|instances, groups| {
            let mut tree = GroupTree::new_with_groups(instances, groups);
            if !tree.group_exists(&persist_path) {
                return Err(GroupError::NotFound(persist_path).into());
            }
            let affected = instances
                .iter()
                .filter(|i| in_group(&i.group_path, &persist_path))
                .count();
            if affected > 0 && !force {
                return Err(GroupError::NotEmpty {
                    path: persist_path,
                    count: affected,
                }
                .into());
            }
            for inst in instances
                .iter_mut()
                .filter(|i| in_group(&i.group_path, &persist_path))
            {
                inst.group_path = String::new();
            }
            tree.delete_group(&persist_path);
            *groups = tree.get_all_groups();
            Ok(affected)
        })
    })
    .await;
    match result {
        Ok(Ok(moved)) => {
            if moved > 0 {
                let mut instances = state.instances.write().await;
                for inst in instances
                    .iter_mut()
                    .filter(|i| i.source_profile == profile && in_group(&i.group_path, &path))
                {
                    inst.group_path = String::new();
                }
            }
            Json(serde_json::json!({ "path": path, "moved_sessions": moved })).into_response()
        }
        Ok(Err(e)) => group_error_response(e, "delete"),
        Err(e) => group_error_response(e.into(), "delete"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_support::build_test_app_state;
    use crate::session::Instance;

    const PROFILE: &str = "group-api";

    struct Fixture {
        _home: tempfile::TempDir,
        _guard: crate::session::test_support::AppDirGuard,
        state: Arc<AppState>,
    }

    fn fixture(sessions: &[(&str, &str)]) -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let guard = crate::session::test_support::isolate_app_dir_at(home.path());
        let _ = crate::session::get_app_dir().expect("isolated app dir");
        let instances: Vec<Instance> = sessions
            .iter()
            .map(|(title, group)| {
                let mut inst = Instance::new(title, "/tmp/p");
                inst.group_path = group.to_string();
                inst.source_profile = PROFILE.to_string();
                inst
            })
            .collect();
        let storage = Storage::new_unwatched(PROFILE).unwrap();
        storage
            .update(|disk, _| {
                *disk = instances.clone();
                Ok(())
            })
            .unwrap();
        Fixture {
            _home: home,
            _guard: guard,
            state: build_test_app_state(instances),
        }
    }

    async fn create(f: &Fixture, name: &str, parent: Option<&str>) -> Response {
        create_group(
            State(f.state.clone()),
            Ok(Json(CreateGroupBody {
                name: name.into(),
                parent: parent.map(Into::into),
                profile: Some(PROFILE.into()),
            })),
        )
        .await
        .into_response()
    }

    async fn delete(f: &Fixture, path: &str, force: bool) -> Response {
        delete_group(
            State(f.state.clone()),
            Ok(Query(DeleteGroupQuery {
                path: path.into(),
                force,
                profile: Some(PROFILE.into()),
            })),
        )
        .await
        .into_response()
    }

    async fn error_code(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        v["error"].as_str().unwrap().to_string()
    }

    fn disk_groups() -> Vec<String> {
        Storage::new_unwatched(PROFILE)
            .unwrap()
            .load_groups_readonly()
            .unwrap()
            .into_iter()
            .map(|g| g.path)
            .collect()
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn create_registers_nested_group_and_rejects_duplicate() {
        let f = fixture(&[]);
        assert_eq!(create(&f, "b", None).await.status(), StatusCode::CREATED);
        assert_eq!(
            create(&f, "a", Some("b")).await.status(),
            StatusCode::CREATED
        );
        assert!(disk_groups().contains(&"b/a".to_string()));

        let dup = create(&f, "a", Some("b")).await;
        assert_eq!(dup.status(), StatusCode::CONFLICT);
        assert_eq!(error_code(dup).await, "group_already_exists");

        let empty = create(&f, "  ", None).await;
        assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
        let unknown = create_group(
            State(f.state.clone()),
            Ok(Json(CreateGroupBody {
                name: "x".into(),
                parent: None,
                profile: Some("no-such-profile".into()),
            })),
        )
        .await
        .into_response();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn delete_needs_force_for_sessions_then_ungroups_disk_and_memory() {
        let f = fixture(&[("one", "team"), ("two", "team/sub"), ("other", "teams")]);
        create(&f, "empty", Some("team")).await;
        assert!(disk_groups().contains(&"team/empty".to_string()));

        let missing = delete(&f, "nope", false).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(error_code(missing).await, "group_not_found");

        let blocked = delete(&f, "team", false).await;
        assert_eq!(blocked.status(), StatusCode::CONFLICT);
        assert_eq!(error_code(blocked).await, "group_not_empty");
        assert!(disk_groups().contains(&"team".to_string()));

        let ok = delete(&f, "team", true).await;
        assert_eq!(ok.status(), StatusCode::OK);
        assert!(disk_groups()
            .iter()
            .all(|p| !p.starts_with("team/") && p != "team"));

        let on_disk = Storage::new_unwatched(PROFILE).unwrap().load().unwrap();
        let live = f.state.instances.read().await;
        for set in [on_disk.as_slice(), live.as_slice()] {
            let group_of = |t: &str| {
                set.iter()
                    .find(|i| i.title == t)
                    .unwrap()
                    .group_path
                    .clone()
            };
            assert_eq!(group_of("one"), "");
            assert_eq!(group_of("two"), "");
            assert_eq!(group_of("other"), "teams", "sibling prefix must stay put");
        }
    }
}
