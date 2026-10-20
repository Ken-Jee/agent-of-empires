//! `aoe serve` must take `serve.lock` before its startup migrations, so it cannot
//! write migrations while `aoe migrate` holds the lock.

use fs2::FileExt;
use serial_test::parallel;

use crate::harness::{app_dir_in, TuiTestHarness};

const VERSION_FILE: &str = ".schema_version";

#[test]
#[parallel]
fn serve_is_refused_before_it_migrates_while_the_lock_is_held() {
    let control = TuiTestHarness::new("serve_lock_control");
    control.run_cli_ok(&["list"]);
    assert!(
        app_dir_in(control.home_path()).join(VERSION_FILE).exists(),
        "an ordinary command must migrate on a fresh home"
    );

    let h = TuiTestHarness::new("serve_lock_held");
    let app_dir = app_dir_in(h.home_path());
    std::fs::create_dir_all(&app_dir).unwrap();
    let lock = std::fs::File::create(app_dir.join("serve.lock")).unwrap();
    lock.try_lock_exclusive().unwrap();

    let stderr = h.run_cli_err(&["serve", "--port", "0"]);
    assert!(stderr.contains("starting or running"), "{stderr}");
    assert!(
        !app_dir.join(VERSION_FILE).exists(),
        "serve ran migrations while the lock was held"
    );
}

#[test]
#[parallel]
fn direct_serve_upgrades_legacy_policy_under_its_existing_transaction() {
    struct Foreground(std::process::Child);
    impl Drop for Foreground {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for detached in [false, true] {
        let mut h = TuiTestHarness::new(if detached {
            "serve_legacy_daemon"
        } else {
            "serve_legacy_foreground"
        });
        h.stop_daemon_on_drop();
        h.run_cli_ok(&["list"]);
        let app = app_dir_in(h.home_path());
        std::fs::write(app.join(VERSION_FILE), "43").unwrap();
        std::fs::write(
            app.join("pending-purge-owners.json"),
            br#"{"version":2,"owners":[]}"#,
        )
        .unwrap();
        let legacy = serde_json::json!({
            "schema":2, "pid":i32::MAX, "instance_id":"legacy-instance",
            "auth_mode":"token", "remote":false, "opaque":{"retained":true}
        });
        for name in ["serve.launch", "serve.rollback.launch"] {
            std::fs::write(app.join(name), serde_json::to_vec(&legacy).unwrap()).unwrap();
        }
        std::fs::write(app.join("serve.passphrase"), "retained rollback secret").unwrap();
        let port = crate::harness::pick_free_port();
        let port_arg = port.to_string();
        let foreground = if detached {
            h.run_cli_ok(&["serve", "--daemon", "--port", &port_arg, "--no-auth"]);
            None
        } else {
            Some(Foreground(h.spawn_cli(&[
                "serve",
                "--port",
                &port_arg,
                "--no-auth",
            ])))
        };
        assert!(
            crate::harness::wait_for_port(port, std::time::Duration::from_secs(10)),
            "direct serve failed to finish its legacy migration (detached={detached})"
        );
        assert_eq!(
            std::fs::read_to_string(app.join(VERSION_FILE)).unwrap(),
            agent_of_empires::migrations::current_schema_version().to_string()
        );
        let rollback: serde_json::Value =
            serde_json::from_slice(&std::fs::read(app.join("serve.rollback.launch")).unwrap())
                .unwrap();
        assert_eq!(rollback["has_passphrase"], true);
        assert_eq!(rollback["auth_mode"], "token");
        assert_eq!(rollback["remote"], false);
        assert_eq!(rollback["pid"], legacy["pid"]);
        assert_eq!(rollback["instance_id"], legacy["instance_id"]);
        assert_eq!(rollback["opaque"], legacy["opaque"]);
        let credential: serde_json::Value =
            serde_json::from_slice(&std::fs::read(app.join("serve.rollback.passphrase")).unwrap())
                .unwrap();
        assert_eq!(credential["pid"], legacy["pid"]);
        assert_eq!(credential["instance_id"], legacy["instance_id"]);
        assert_eq!(credential["passphrase"], "retained rollback secret");
        let status = h.run_cli(&["serve", "--status"]);
        assert!(
            status.status.success(),
            "upgraded daemon must remain discoverable: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        let status = String::from_utf8_lossy(&status.stdout);
        assert!(status.contains("Daemon: running (PID "), "{status}");
        assert!(status.contains(&format!(":{port}")), "{status}");
        h.run_cli_ok(&["serve", "--stop"]);
        drop(foreground);
    }
}
