//! 取号、Continue、批量轮询、终态核销与 3102 续跑。
//!
//! 真源：`packages/services/src/session/offPeakTaskService.ts` 的 server 相关方法。
//! 定时器由调用方驱动：`run_sync_cycle` 返回下一次建议延迟。

use off_peak_repo::{CreateOptions, CreateParams, InvalidateOptions, SchedulingPatch};
use serde_json::Value;

use crate::server::{
    OffPeakClientError, OffPeakTaskCreateFailure, OffPeakTaskCreateParams, OffPeakTaskCreateResult,
    OffPeakTaskCreateSuccess,
};
use crate::{
    OffPeakTaskEnvironment, OffPeakTaskService, OFF_PEAK_SYNC_MAX_INTERVAL_MS,
    OFF_PEAK_SYNC_MIN_INTERVAL_MS, SYNC_FAILURE_BASE_MS,
};

const VALID_PERMISSION_MODES: [&str; 6] = ["yolo", "plan", "edit", "auto", "autoEdit", "build"];
const OFF_PEAK_SESSION_BOUND_STAGE: &str = "client_validation";
const OFF_PEAK_SESSION_BOUND_CODE: &str = "session_bound";

fn workspace_key(path: &str, identity: Option<&str>) -> String {
    identity
        .map(str::trim)
        .filter(|identity| !identity.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn is_valid_create_params(params: &OffPeakTaskCreateParams) -> bool {
    !params.title.trim().is_empty()
        && !params.prompt.trim().is_empty()
        && !params.workspace_path.trim().is_empty()
        && VALID_PERMISSION_MODES.contains(&params.permission_mode.as_str())
}

fn classify_client_error(error: &OffPeakClientError) -> (&'static str, String) {
    match error {
        OffPeakClientError::Server { biz_code, .. } => match biz_code {
            Some(3101) => ("eligibility_3101", "3101".to_string()),
            Some(3103) => ("quota_3103", "3103".to_string()),
            Some(code) => ("unknown", code.to_string()),
            None => ("unknown", String::new()),
        },
        OffPeakClientError::InvalidResponse => ("invalid_response", String::new()),
        OffPeakClientError::Network => ("network", String::new()),
    }
}

fn failure(
    provider_name: &str,
    failure_stage: &str,
    error_category: &str,
    error_code: &str,
) -> OffPeakTaskCreateResult {
    OffPeakTaskCreateResult::Failure(OffPeakTaskCreateFailure {
        ok: false,
        failure_stage: failure_stage.to_string(),
        error_category: error_category.to_string(),
        error_code: error_code.to_string(),
        provider_name: provider_name.to_string(),
    })
}

fn session_bound_failure(provider_name: &str) -> OffPeakTaskCreateResult {
    failure(
        provider_name,
        OFF_PEAK_SESSION_BOUND_STAGE,
        OFF_PEAK_SESSION_BOUND_STAGE,
        OFF_PEAK_SESSION_BOUND_CODE,
    )
}

impl<E: OffPeakTaskEnvironment> OffPeakTaskService<E> {
    pub fn get_coding_plan_support(&self) -> Value {
        self.environment.coding_plan_support()
    }

    pub fn get_take_number_availability(&self) -> Result<Value, String> {
        let Some(client) = &self.client else {
            return Err("off-peak server client not configured".to_string());
        };
        client
            .get_take_number_availability()
            .map(|availability| serde_json::to_value(availability).expect("序列化"))
            .map_err(|_| "off-peak availability request failed".to_string())
    }

    pub fn create_task(
        &self,
        params: &OffPeakTaskCreateParams,
    ) -> Result<OffPeakTaskCreateResult, String> {
        let provider_name = self
            .environment
            .telemetry_provider_name()
            .unwrap_or_default();
        if !is_valid_create_params(params) {
            return Ok(failure(
                &provider_name,
                "client_validation",
                "client_validation",
                "",
            ));
        }
        let Some(resolved) = self
            .environment
            .resolve_model_selection(&params.model_selection)
        else {
            return Ok(failure(
                &provider_name,
                "client_validation",
                "client_validation",
                "",
            ));
        };
        let normalized = CreateParams {
            title: params.title.clone(),
            prompt: params.prompt.clone(),
            permission_mode: params.permission_mode.clone(),
            model_selection: resolved,
            workspace_path: params.workspace_path.clone(),
            workspace_identity: params.workspace_identity.clone(),
            bound_session_id: params.bound_session_id.clone(),
        };
        if let Some(bound_session_id) = params.bound_session_id.as_deref() {
            let key = workspace_key(&params.workspace_path, params.workspace_identity.as_deref());
            if self
                .repo
                .has_active_bound_task(&key, bound_session_id)
                .map_err(|error| error.to_string())?
            {
                return Ok(session_bound_failure(&provider_name));
            }
        }
        let Some(client) = &self.client else {
            return Ok(failure(&provider_name, "ticket_request", "network", ""));
        };
        let task_id = (self.new_task_id)();
        let ticket = match client.take_ticket(&task_id) {
            Ok(ticket) => ticket,
            Err(error) => {
                let (category, code) = classify_client_error(&error);
                return Ok(failure(&provider_name, "ticket_request", category, &code));
            }
        };
        let created = match self.repo.create(
            &normalized,
            &CreateOptions {
                now: None,
                off_peak_task_id: Some(task_id),
                server_ticket_id: Some(ticket.ticket_id.clone()),
                queue_position: ticket.position,
                registered_at: Some(ticket.registered_at),
                schedulable: Some(ticket.state == "ready"),
            },
        ) {
            Ok(created) => created,
            Err(error) => {
                let message = error.to_string();
                if message.contains(
                    "UNIQUE constraint failed: off_peak_tasks.workspace_key, off_peak_tasks.session_id",
                ) {
                    return Ok(session_bound_failure(&provider_name));
                }
                return Ok(failure(
                    &provider_name,
                    "local_persist",
                    "local_persist",
                    "",
                ));
            }
        };
        if let Some(next_poll_after_ms) = ticket.next_poll_after_ms {
            if self
                .repo
                .update_scheduling_snapshot(
                    &created.off_peak_task_id,
                    &SchedulingPatch {
                        next_poll_at: Some(Some(self.now() + next_poll_after_ms)),
                        ..SchedulingPatch::default()
                    },
                )
                .is_err()
            {
                return Ok(failure(
                    &provider_name,
                    "local_persist",
                    "local_persist",
                    "",
                ));
            }
        }
        self.emit_changed();
        if ticket.state == "ready" {
            self.environment.request_scheduler_wake();
        }
        Ok(OffPeakTaskCreateResult::Success(Box::new(
            OffPeakTaskCreateSuccess {
                ok: true,
                task: created,
                ticket_initial_state: ticket.state,
                queue_position: ticket.position,
                provider_name,
            },
        )))
    }

    pub fn continue_task(&self, off_peak_task_id: &str) -> Result<Option<Value>, String> {
        let resumed = self
            .repo
            .set_paused(
                off_peak_task_id,
                false,
                &InvalidateOptions {
                    now: Some(self.now()),
                },
            )
            .map_err(|error| error.to_string())?;
        let Some(resumed) = resumed else {
            return Ok(None);
        };
        let Some(client) = &self.client else {
            return Err("off-peak server client not configured".to_string());
        };
        let mut ticket_alive = false;
        if let Some(ticket_id) = resumed.server_ticket_id.as_deref() {
            match client.batch_status(&[ticket_id.to_string()]) {
                Ok(status) => {
                    let entry = status
                        .tickets
                        .iter()
                        .find(|entry| entry.ticket_id == ticket_id);
                    ticket_alive = entry.is_some_and(|entry| {
                        entry.state != "expired" && entry.state != "not_found"
                    });
                    if let Some(entry) = entry.filter(|_| ticket_alive) {
                        self.repo
                            .update_scheduling_snapshot(
                                off_peak_task_id,
                                &SchedulingPatch {
                                    schedulable: Some(entry.state == "ready"),
                                    queue_position: Some(entry.position),
                                    now: Some(self.now()),
                                    ..SchedulingPatch::default()
                                },
                            )
                            .map_err(|error| error.to_string())?;
                    }
                }
                // 状态查询失败按票活处理，交给轮询兜底；不白耗一次取号额度。
                Err(_) => ticket_alive = true,
            }
        }
        if !ticket_alive {
            self.retake_ticket(off_peak_task_id, client.as_ref());
        }
        self.emit_changed();
        self.repo
            .get(off_peak_task_id)
            .map(|task| task.map(|task| serde_json::to_value(task).expect("序列化")))
            .map_err(|error| error.to_string())
    }

    pub fn handle_ticket_expired_during_run(&self, off_peak_task_id: &str) -> Result<(), String> {
        let requeued = self
            .repo
            .requeue_for_continuation(
                off_peak_task_id,
                &InvalidateOptions {
                    now: Some(self.now()),
                },
            )
            .map_err(|error| error.to_string())?;
        if requeued.is_none() {
            return Ok(());
        }
        if let Some(client) = &self.client {
            self.retake_ticket(off_peak_task_id, client.as_ref());
        }
        self.emit_changed();
        Ok(())
    }

    fn retake_ticket(&self, off_peak_task_id: &str, client: &dyn crate::OffPeakServerClient) {
        let Ok(ticket) = client.take_ticket(off_peak_task_id) else {
            return;
        };
        let _ = self.repo.update_scheduling_snapshot(
            off_peak_task_id,
            &SchedulingPatch {
                server_ticket_id: Some(ticket.ticket_id),
                registered_at: Some(ticket.registered_at),
                schedulable: Some(ticket.state == "ready"),
                queue_position: Some(ticket.position),
                next_poll_at: ticket
                    .next_poll_after_ms
                    .map(|delay| Some(self.now() + delay)),
                now: Some(self.now()),
            },
        );
        if ticket.state == "ready" {
            self.environment.request_scheduler_wake();
        }
    }

    pub fn start_sync(&self) {
        self.sync_state.lock().expect("sync state lock").stopped = false;
    }

    pub fn stop_sync(&self) {
        self.sync_state.lock().expect("sync state lock").stopped = true;
    }

    pub fn is_sync_stopped(&self) -> bool {
        self.sync_state.lock().expect("sync state lock").stopped
    }

    pub fn dispose_all(&self) {
        self.stop_sync();
        self.environment.on_dispose();
    }

    /// 单次同步周期；返回下一次建议延迟（毫秒，已钳制）。调用方负责定时器。
    pub fn run_sync_cycle(&self) -> i64 {
        {
            let mut state = self.sync_state.lock().expect("sync state lock");
            if state.running {
                return OFF_PEAK_SYNC_MIN_INTERVAL_MS;
            }
            state.running = true;
        }
        let mut next_delay = OFF_PEAK_SYNC_MAX_INTERVAL_MS;
        let outcome = self.run_sync_cycle_inner(&mut next_delay);
        let mut state = self.sync_state.lock().expect("sync state lock");
        state.running = false;
        match outcome {
            Ok(()) => state.consecutive_failures = 0,
            Err(_) => {
                state.consecutive_failures += 1;
                let exponent = (state.consecutive_failures - 1).max(0) as u32;
                next_delay =
                    (SYNC_FAILURE_BASE_MS * 2_i64.pow(exponent)).min(OFF_PEAK_SYNC_MAX_INTERVAL_MS);
            }
        }
        next_delay.clamp(OFF_PEAK_SYNC_MIN_INTERVAL_MS, OFF_PEAK_SYNC_MAX_INTERVAL_MS)
    }

    fn run_sync_cycle_inner(&self, next_delay: &mut i64) -> Result<(), String> {
        self.flush_settle_outbox();
        let non_terminal = self
            .repo
            .list_non_terminal()
            .map_err(|error| error.to_string())?;
        let with_tickets: Vec<_> = non_terminal
            .iter()
            .filter(|task| task.server_ticket_id.is_some())
            .cloned()
            .collect();
        if let Some(client) = &self.client {
            for task in &non_terminal {
                if task.server_ticket_id.is_none() && task.status == "queued" {
                    self.retake_ticket(&task.off_peak_task_id, client.as_ref());
                }
            }
        }
        if with_tickets.is_empty() && non_terminal.is_empty() {
            return Ok(());
        }
        if with_tickets.is_empty() {
            *next_delay = OFF_PEAK_SYNC_MIN_INTERVAL_MS;
            return Ok(());
        }
        let Some(client) = &self.client else {
            return Err("off-peak server client not configured".to_string());
        };
        let ticket_ids: Vec<String> = with_tickets
            .iter()
            .filter_map(|task| task.server_ticket_id.clone())
            .collect();
        let status = client
            .batch_status(&ticket_ids)
            .map_err(|_| "off-peak batch status failed".to_string())?;
        let mut any_became_schedulable = false;
        for task in &with_tickets {
            let Some(ticket_id) = task.server_ticket_id.as_deref() else {
                continue;
            };
            let Some(entry) = status
                .tickets
                .iter()
                .find(|entry| entry.ticket_id == ticket_id)
            else {
                continue;
            };
            any_became_schedulable = self
                .apply_ticket_status(task, &entry.state, entry.position)
                .map_err(|error| error.to_string())?
                || any_became_schedulable;
        }
        if any_became_schedulable {
            self.environment.request_scheduler_wake();
        }
        *next_delay = status
            .next_poll_after_ms
            .unwrap_or(OFF_PEAK_SYNC_MIN_INTERVAL_MS);
        self.emit_changed();
        Ok(())
    }

    fn apply_ticket_status(
        &self,
        task: &off_peak_repo::OffPeakTask,
        state: &str,
        position: Option<i64>,
    ) -> Result<bool, off_peak_repo::OffPeakRepoError> {
        let now = self.now();
        match state {
            "ready" => {
                let became = !task.schedulable && task.status == "queued";
                self.repo.update_scheduling_snapshot(
                    &task.off_peak_task_id,
                    &SchedulingPatch {
                        schedulable: Some(true),
                        queue_position: Some(position),
                        next_poll_at: Some(Some(now + OFF_PEAK_SYNC_MIN_INTERVAL_MS)),
                        now: Some(now),
                        ..SchedulingPatch::default()
                    },
                )?;
                Ok(became)
            }
            "queued" => {
                self.repo.update_scheduling_snapshot(
                    &task.off_peak_task_id,
                    &SchedulingPatch {
                        schedulable: Some(false),
                        queue_position: Some(position),
                        now: Some(now),
                        ..SchedulingPatch::default()
                    },
                )?;
                Ok(false)
            }
            "active" => {
                self.repo.update_scheduling_snapshot(
                    &task.off_peak_task_id,
                    &SchedulingPatch {
                        queue_position: Some(None),
                        now: Some(now),
                        ..SchedulingPatch::default()
                    },
                )?;
                Ok(false)
            }
            "expired" => {
                if task.status == "queued" {
                    self.repo.update_scheduling_snapshot(
                        &task.off_peak_task_id,
                        &SchedulingPatch {
                            schedulable: Some(false),
                            queue_position: Some(None),
                            now: Some(now),
                            ..SchedulingPatch::default()
                        },
                    )?;
                    if let Some(client) = &self.client {
                        self.retake_ticket(&task.off_peak_task_id, client.as_ref());
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn flush_settle_outbox(&self) {
        let Ok(unsettled) = self.repo.list_unsettled_terminal() else {
            return;
        };
        for task in unsettled {
            self.settle_one(&task);
        }
    }

    fn settle_one(&self, task: &off_peak_repo::OffPeakTask) {
        let Some(ticket_id) = task.server_ticket_id.as_deref() else {
            let _ = self.repo.mark_settled(&task.off_peak_task_id, self.now());
            return;
        };
        let Some(client) = &self.client else {
            return;
        };
        match client.settle(ticket_id) {
            Ok(()) => {
                let _ = self.repo.mark_settled(&task.off_peak_task_id, self.now());
            }
            Err(OffPeakClientError::Server { http_status, .. }) if http_status < 500 => {
                let _ = self.repo.mark_settled(&task.off_peak_task_id, self.now());
            }
            Err(_) => {}
        }
    }
}
