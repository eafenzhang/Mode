//! automation-service：list / setEnabled / runNow / listRuns / deleteRun / restart / delete。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use automation_repo::{AutomationCreateOptions, AutomationCreateParams, AutomationRepo, AutomationScope};
use automation_service::{AutomationService, AutomationServiceOptions};

struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-automation-service-{}-{}.sqlite",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-wal"));
    }
}

fn seed(repo: &AutomationRepo, id_title: &str) -> String {
    let automation = repo
        .create(
            &AutomationCreateParams {
                title: id_title.to_string(),
                cron_expr: "0 9 * * *".to_string(),
                prompt: "run".to_string(),
                model_selection: None,
                mode: None,
                workspace_path: "/ws/demo".to_string(),
                workspace_identity: None,
                target_task_id: None,
                bot_delivery_target: None,
                recurring: true,
                max_runs: None,
                end_at: None,
                schedule_rule: None,
            },
            &AutomationCreateOptions {
                next_run_at: Some(1_700_000_000_000),
                lifecycle_status: None,
            },
        )
        .expect("create automation");
    automation.automation_id
}

#[test]
fn list_set_enabled_run_now_and_delete() {
    let db = TempDb::new();
    let repo = AutomationRepo::new(&db.0);
    let automation_id = seed(&repo, "Morning");
    let service = AutomationService::new(
        AutomationRepo::new(&db.0),
        AutomationServiceOptions { offset_secs: 0 },
    );
    let scope = AutomationScope {
        workspace_path: Some("/ws/demo".to_string()),
        workspace_identity: None,
    };

    let listed = service.list(&scope).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Morning");
    assert_eq!(listed[0].next_run_at, Some(1_700_000_000_000));
    assert!(listed[0].schedule_rule.is_none());
    assert_eq!(service.list_all().expect("list all").len(), 1);

    service
        .set_enabled(&automation_id, false, &scope)
        .expect("set enabled");
    let stored = repo
        .get(&automation_id, Some("/ws/demo"))
        .expect("get")
        .expect("exists");
    assert!(!stored.enabled);

    let run = service
        .run_now(&automation_id, &scope, 1_700_000_100_000)
        .expect("run now")
        .expect("claimed");
    assert_eq!(run.run.trigger, "manual");
    assert_eq!(run.automation.automation_id, automation_id);
    let runs = service.list_runs(&automation_id, &scope).expect("list runs");
    assert_eq!(runs.len(), 1);
    service.delete_run(&runs[0].run_id, &scope).expect("delete run");
    assert!(service
        .list_runs(&automation_id, &scope)
        .expect("list runs")
        .is_empty());

    assert!(service.delete(&automation_id, &scope).expect("delete"));
    assert!(service.list(&scope).expect("list").is_empty());
}

#[test]
fn restart_only_recovers_failed_automations() {
    let db = TempDb::new();
    let repo = AutomationRepo::new(&db.0);
    let automation = repo
        .create(
            &AutomationCreateParams {
                title: "Failed".to_string(),
                cron_expr: "0 9 * * *".to_string(),
                prompt: "run".to_string(),
                model_selection: None,
                mode: None,
                workspace_path: "/ws/demo".to_string(),
                workspace_identity: None,
                target_task_id: None,
                bot_delivery_target: None,
                recurring: true,
                max_runs: None,
                end_at: None,
                schedule_rule: None,
            },
            &AutomationCreateOptions {
                next_run_at: None,
                lifecycle_status: Some("failed".to_string()),
            },
        )
        .expect("create failed automation");
    let service = AutomationService::new(
        AutomationRepo::new(&db.0),
        AutomationServiceOptions { offset_secs: 0 },
    );
    let scope = AutomationScope {
        workspace_path: Some("/ws/demo".to_string()),
        workspace_identity: None,
    };
    service
        .restart(&automation.automation_id, &scope, 1_700_000_100_000)
        .expect("restart");
    let stored = repo
        .get(&automation.automation_id, Some("/ws/demo"))
        .expect("get")
        .expect("exists");
    assert_eq!(stored.lifecycle_status, "active");
    assert!(stored.next_run_at.is_some());
}
