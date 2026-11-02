//! 定时任务管理服务（`AutomationService` 的 Rust 侧）。
//!
//! 真源：`packages/services/src/session/automationService.ts`。承载 create/update 的
//! 调度语义（relative delay、interval carrier、minute 锚点、cron 变更清规则、生命周期
//! 重算）与 list/delete/setEnabled/restart/runNow/listRuns/deleteRun。
//!
//! 时钟与时区偏移由调用方注入：`create/update/restart/runNow` 都接受 `now`，
//! `offset_secs` 在构造时固定（与 `automation-schedule` 口径一致）。

mod params;
mod types;

use automation_repo::{
    AutomationCreateOptions, AutomationCreateParams, AutomationRepo, AutomationRestartOptions,
    AutomationScope, AutomationUpdateOptions, AutomationUpdateParams, RunNowOptions,
};
use automation_schedule::{
    assert_valid_automation_interval_carrier, build_interval_schedule_rule,
    build_relative_delay_schedule, compute_automation_next_run_at,
    compute_initial_automation_next_run_at, infer_minute_interval_schedule_rule,
    is_valid_cron_expr, schedule_rule_definition, IntervalUnit, MonthlyMode, ScheduleRule,
};
use serde_json::{Map, Value};

pub use params::{AutomationCreateParams as CreateParams, AutomationUpdateParams as UpdateParams};
pub use types::{ModeAutomation, ModeAutomationRun, ModeAutomationRunNow};

const MAX_MONTHLY_AUTOMATION_SCHEDULE_INTERVAL: f64 = 1_200.0;

pub struct AutomationServiceOptions {
    pub offset_secs: i32,
}

pub struct AutomationService {
    repo: AutomationRepo,
    offset_secs: i32,
}

impl AutomationService {
    pub fn new(repo: AutomationRepo, options: AutomationServiceOptions) -> Self {
        Self {
            repo,
            offset_secs: options.offset_secs,
        }
    }

    /// `create`：carrier/relative delay 归一化 → 规则校验 → 首次触发时间 → 写库。
    pub fn create(
        &self,
        params: &CreateParams,
        scope: &AutomationScope,
        now: i64,
    ) -> Result<ModeAutomation, String> {
        let workspace_path = scope
            .workspace_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "missing workspacePath".to_string())?;
        let relative_delay = params.relative_delay_minutes;
        if let Some(delay) = relative_delay {
            if !delay.is_finite() || delay.fract() != 0.0 || !(1.0..=525_600.0).contains(&delay) {
                return Err(
                    "非法的相对时间定时任务：delayMinutes 必须是 1-525600 的整数".to_string(),
                );
            }
            if params.recurring == Some(true) {
                return Err(
                    "非法的相对时间定时任务：相对延迟任务必须设置 recurring=false".to_string(),
                );
            }
            if params.max_runs.is_some() {
                return Err(
                    "非法的相对时间定时任务：相对延迟任务固定只运行一次，不能设置 maxRuns"
                        .to_string(),
                );
            }
            if params.schedule_rule.is_some() {
                return Err(
                    "非法的相对时间定时任务：不能同时提交 delayMinutes 和 scheduleRule".to_string(),
                );
            }
        }
        assert_valid_automation_interval_carrier(&carrier_input(params))
            .map_err(|error| error.to_string())?;

        let mut cron_expr = params.cron_expr.clone();
        let mut recurring = params.recurring.unwrap_or(false);
        let mut max_runs = params.max_runs;
        let mut schedule_rule = params.schedule_rule.clone();
        if let Some(delay) = relative_delay {
            let (relative_cron, relative_rule) =
                build_relative_delay_schedule(delay, now, self.offset_secs);
            cron_expr = relative_cron;
            schedule_rule = Some(relative_rule);
            recurring = false;
        }
        if let (Some(unit), Some(interval)) = (params.interval_unit, params.interval) {
            recurring = true;
            max_runs = None;
            schedule_rule = Some(build_interval_schedule_rule(
                unit,
                interval,
                &cron_expr,
                now,
                self.offset_secs,
            ));
        }
        if !is_valid_cron_expr(&cron_expr) {
            return Err(format!("非法的 cron 表达式：{cron_expr}"));
        }
        if let Some(rule) = &schedule_rule {
            assert_valid_schedule_rule(rule)?;
        }
        let schedule_rule = match schedule_rule {
            Some(rule) => Some(ScheduleRule {
                anchor_at: now,
                ..rule
            }),
            None => infer_minute_interval_schedule_rule(&cron_expr, now, self.offset_secs),
        };
        let next_run_at = compute_initial_automation_next_run_at(
            &cron_expr,
            recurring,
            schedule_rule.as_ref(),
            now,
            self.offset_secs,
        )
        .map_err(|error| error.to_string())?;
        let ended_before_first_run = params
            .end_at
            .is_some_and(|end_at| next_run_at.unwrap_or(i64::MAX) > end_at);

        let created = self
            .repo
            .create(
                &AutomationCreateParams {
                    title: params.title.clone(),
                    cron_expr,
                    prompt: params.prompt.clone(),
                    model_selection: params
                        .model_selection
                        .as_ref()
                        .map(|selection| selection.to_repo()),
                    mode: params.mode.clone(),
                    workspace_path: workspace_path.to_string(),
                    workspace_identity: scope.workspace_identity.clone(),
                    target_task_id: params.target_task_id.clone(),
                    bot_delivery_target: params
                        .bot_delivery_target
                        .as_ref()
                        .map(|target| target.to_repo()),
                    recurring,
                    max_runs,
                    end_at: params.end_at,
                    schedule_rule: schedule_rule
                        .as_ref()
                        .map(|rule| serde_json::to_value(rule).unwrap_or(Value::Null)),
                },
                &AutomationCreateOptions {
                    next_run_at: if ended_before_first_run {
                        None
                    } else {
                        next_run_at
                    },
                    lifecycle_status: ended_before_first_run.then(|| "completed".to_string()),
                },
            )
            .map_err(|error| error.to_string())?;
        Ok(ModeAutomation::from(&created))
    }

    /// `update`：规则变更、cron 变更、endAt 与 recurring/maxRuns 的生命周期重算。
    pub fn update(
        &self,
        automation_id: &str,
        params: &UpdateParams,
        scope: &AutomationScope,
        now: i64,
    ) -> Result<Option<ModeAutomation>, String> {
        let workspace_key = scope_key(scope);
        let Some(existing) = self
            .repo
            .get(automation_id, workspace_key.as_deref())
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };

        // `scheduleRule: null` 是显式清除，不能与「未提交」混为一谈。
        let direct_schedule = params.schedule_rule.clone();
        if let Some(Some(rule)) = &direct_schedule {
            assert_valid_schedule_rule(rule)?;
        }
        assert_valid_automation_interval_carrier(&carrier_input_for_update(params))
            .map_err(|error| error.to_string())?;
        let has_interval_carrier = params.interval_unit.is_some() && params.interval.is_some();

        if !has_interval_carrier
            && matches!(params.max_runs, Some(None))
            && params.recurring != Some(true)
        {
            return Err("清空 maxRuns 时必须在同一次更新中设置 recurring=true".to_string());
        }
        let next_recurring = if has_interval_carrier {
            true
        } else {
            params.recurring.unwrap_or(existing.recurring)
        };
        if next_recurring && matches!(params.max_runs, Some(Some(_))) {
            return Err("无限循环任务不能设置有限次数 maxRuns".to_string());
        }

        // 有限任务只提交 recurring=true 时旧 maxRuns 会被原样保留，形成矛盾隐藏状态；
        // 领域层统一补 null，原子清除旧上限。
        let normalized_recurring = if has_interval_carrier {
            Some(true)
        } else {
            params.recurring
        };
        let normalized_max_runs = if has_interval_carrier
            || (next_recurring
                && params.max_runs.is_none()
                && (params.recurring == Some(true) || existing.max_runs.is_some()))
        {
            Some(None)
        } else {
            params.max_runs
        };

        let mut update = AutomationUpdateParams {
            title: params.title.clone(),
            cron_expr: params.cron_expr.clone(),
            prompt: params.prompt.clone(),
            model_selection: params
                .model_selection
                .clone()
                .map(|value| value.map(|selection| selection.to_repo())),
            mode: params.mode.clone(),
            recurring: normalized_recurring,
            max_runs: normalized_max_runs,
            end_at: params.end_at,
            schedule_rule: None,
            schedule_edited_by_user: params.schedule_edited_by_user,
        };

        if let Some(cron_expr) = &params.cron_expr {
            if !is_valid_cron_expr(cron_expr) {
                return Err(format!("非法的 cron 表达式：{cron_expr}"));
            }
        }
        let cron_changed = params
            .cron_expr
            .as_ref()
            .is_some_and(|cron_expr| cron_expr != &existing.cron_expr);
        let effective_cron = params
            .cron_expr
            .clone()
            .unwrap_or_else(|| existing.cron_expr.clone());
        let existing_rule = types::parse_schedule_rule(existing.schedule_rule.as_ref());
        // carrier 提交即代表用户要重设间隔：统一以本次保存时刻重新锚定。
        let carrier_rule = if has_interval_carrier {
            Some(build_interval_schedule_rule(
                params.interval_unit.expect("checked carrier unit"),
                params.interval.expect("checked carrier interval"),
                &effective_cron,
                now,
                self.offset_secs,
            ))
        } else {
            None
        };
        let direct_defined = direct_schedule.is_some();
        // carrier 与直传 scheduleRule 都算「显式规则已提交」（null 清除也算）。
        let explicit_defined = direct_defined || carrier_rule.is_some();
        let explicit_input = match &direct_schedule {
            Some(value) => value.clone(),
            None => carrier_rule,
        };
        if let Some(rule) = &explicit_input {
            assert_valid_schedule_rule(rule)?;
        }
        let explicit_rule = explicit_input.map(|rule| ScheduleRule {
            anchor_at: match &existing_rule {
                Some(existing_rule)
                    if schedule_rule_definition(existing_rule)
                        == schedule_rule_definition(&rule) =>
                {
                    existing_rule.anchor_at
                }
                _ => now,
            },
            ..rule
        });
        // cron 改变但没有显式规则时，旧规则不能继续覆盖新 cron：分钟间隔重新锚定，
        // 其它 cron 清空规则并恢复日历语义。
        let inferred_rule = if cron_changed && !explicit_defined {
            infer_minute_interval_schedule_rule(&effective_cron, now, self.offset_secs)
        } else {
            None
        };
        let effective_rule = if !explicit_defined {
            if cron_changed {
                inferred_rule.clone()
            } else {
                existing_rule.clone()
            }
        } else {
            explicit_rule.clone()
        };

        // 只有 cron 或规则本身变化才改写 schedule_rule；仅改 endAt 时保留原文本
        // （含无法解析的历史规则），与 TS updateParams 的写入条件一致。
        if cron_changed || explicit_defined {
            update.schedule_rule = Some(
                effective_rule
                    .as_ref()
                    .map(|rule| serde_json::to_value(rule).unwrap_or(Value::Null)),
            );
        }

        let mut options = AutomationUpdateOptions::default();
        if cron_changed || params.end_at.is_some() || explicit_defined {
            options.next_run_at = Some(compute_automation_next_run_at(
                &effective_cron,
                effective_rule.as_ref(),
                now,
                self.offset_secs,
            ));
            options.reset_retry = true;
        }

        let effective_end_at = match params.end_at {
            Some(value) => value,
            None => existing.end_at,
        };
        if let Some(end_at) = effective_end_at {
            let next = options
                .next_run_at
                .as_ref()
                .copied()
                .flatten()
                .or(existing.next_run_at);
            if next.unwrap_or(i64::MAX) > end_at {
                options.next_run_at = Some(None);
                options.lifecycle_status = Some("completed".to_string());
            }
        } else if params.end_at.is_some()
            && (existing.lifecycle_status == "completed" || existing.lifecycle_status == "failed")
        {
            options.lifecycle_status = Some("active".to_string());
        }

        if normalized_recurring.is_some() || normalized_max_runs.is_some() {
            let next_max_runs = match normalized_max_runs {
                Some(value) => value,
                None => existing.max_runs,
            };
            let scheduled_run_count = self
                .repo
                .get_scheduled_run_count(automation_id, workspace_key.as_deref())
                .map_err(|error| error.to_string())?;
            let Some(scheduled_run_count) = scheduled_run_count else {
                return Ok(None);
            };
            let reached_max = !next_recurring && scheduled_run_count >= next_max_runs.unwrap_or(1);
            if reached_max {
                options.lifecycle_status = Some("completed".to_string());
            } else if existing.lifecycle_status == "completed"
                || existing.lifecycle_status == "failed"
            {
                // 原为终态，编辑后又变得可跑 → 复活为 active 并重算 next_run_at。
                options.lifecycle_status = Some("active".to_string());
                if options.next_run_at.is_none() {
                    options.next_run_at = Some(compute_automation_next_run_at(
                        &effective_cron,
                        effective_rule.as_ref(),
                        now,
                        self.offset_secs,
                    ));
                }
            }
        }

        self.repo
            .update(automation_id, &update, &options, workspace_key.as_deref())
            .map(|result| result.as_ref().map(ModeAutomation::from))
            .map_err(|error| error.to_string())
    }

    pub fn list(&self, scope: &AutomationScope) -> Result<Vec<ModeAutomation>, String> {
        self.repo
            .list(scope)
            .map(|items| items.iter().map(ModeAutomation::from).collect())
            .map_err(|error| error.to_string())
    }

    pub fn list_all(&self) -> Result<Vec<ModeAutomation>, String> {
        self.list(&AutomationScope::default())
    }

    pub fn delete(&self, automation_id: &str, scope: &AutomationScope) -> Result<bool, String> {
        self.repo
            .delete(automation_id, scope_key(scope).as_deref())
            .map_err(|error| error.to_string())
    }

    pub fn set_enabled(
        &self,
        automation_id: &str,
        enabled: bool,
        scope: &AutomationScope,
    ) -> Result<(), String> {
        self.repo
            .set_enabled(automation_id, enabled, scope_key(scope).as_deref())
            .map_err(|error| error.to_string())
    }

    /// 失败任务手动重跑：仅 `failed` 可恢复；按 cron/规则重算下次时间。
    pub fn restart(
        &self,
        automation_id: &str,
        scope: &AutomationScope,
        now: i64,
    ) -> Result<(), String> {
        let workspace_key = scope_key(scope);
        let Some(existing) = self
            .repo
            .get(automation_id, workspace_key.as_deref())
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if existing.lifecycle_status != "failed" {
            return Ok(());
        }
        let schedule_rule = types::parse_schedule_rule(existing.schedule_rule.as_ref());
        let next_run_at = compute_automation_next_run_at(
            &existing.cron_expr,
            schedule_rule.as_ref(),
            now,
            self.offset_secs,
        );
        self.repo
            .restart(
                automation_id,
                &AutomationRestartOptions { next_run_at },
                workspace_key.as_deref(),
            )
            .map_err(|error| error.to_string())
    }

    /// 立即运行：写 manual run 并占用 single-flight claim，不修改 cron 计划。
    pub fn run_now(
        &self,
        automation_id: &str,
        scope: &AutomationScope,
        now: i64,
    ) -> Result<Option<ModeAutomationRunNow>, String> {
        let workspace_key = scope_key(scope);
        let claimed = self
            .repo
            .run_now(
                automation_id,
                &RunNowOptions { now },
                workspace_key.as_deref(),
            )
            .map_err(|error| error.to_string())?;
        Ok(claimed.map(|claimed| ModeAutomationRunNow {
            automation: ModeAutomation::from(&claimed.automation),
            run: ModeAutomationRun::from(&claimed.run),
        }))
    }

    pub fn list_runs(
        &self,
        automation_id: &str,
        scope: &AutomationScope,
    ) -> Result<Vec<ModeAutomationRun>, String> {
        self.repo
            .list_runs(automation_id, scope_key(scope).as_deref())
            .map(|items| items.iter().map(ModeAutomationRun::from).collect())
            .map_err(|error| error.to_string())
    }

    pub fn delete_run(&self, run_id: &str, scope: &AutomationScope) -> Result<(), String> {
        self.repo
            .delete_run(run_id, scope_key(scope).as_deref())
            .map_err(|error| error.to_string())
    }

    pub fn repo(&self) -> &AutomationRepo {
        &self.repo
    }
}

/// TS `assertValidAutomationScheduleRule` 的等价校验；写库前必须拒绝非法规则。
fn assert_valid_schedule_rule(rule: &ScheduleRule) -> Result<(), String> {
    let error = |message: &str| Err(format!("非法的定时任务调度规则：{message}"));
    if !rule.interval.is_finite() || rule.interval.fract() != 0.0 || rule.interval < 1.0 {
        return error("interval 必须是正整数");
    }
    if rule.unit == IntervalUnit::Monthly
        && rule.interval > MAX_MONTHLY_AUTOMATION_SCHEDULE_INTERVAL
    {
        return error("monthly interval 不能超过 1200");
    }
    if !(0..=23).contains(&rule.hour) {
        return error("hour 必须是 0-23 的整数");
    }
    if !(0..=59).contains(&rule.minute) {
        return error("minute 必须是 0-59 的整数");
    }
    if rule
        .weekdays
        .as_ref()
        .is_some_and(|values| !valid_values(values, 0, 6))
    {
        return error("weekdays 必须是 0-6 的非空整数数组");
    }
    if rule.unit == IntervalUnit::Weekly
        && rule
            .weekdays
            .as_ref()
            .is_none_or(|values| values.is_empty())
    {
        return error("weekly 规则必须包含有效的 weekdays");
    }
    if rule.unit == IntervalUnit::Monthly {
        if rule.monthly_mode == Some(MonthlyMode::Weekday) {
            if rule
                .weekdays
                .as_ref()
                .is_none_or(|values| !valid_values(values, 0, 6))
            {
                return error("monthly weekday 规则必须包含有效的 weekdays");
            }
        } else if rule
            .month_days
            .as_ref()
            .is_none_or(|values| !valid_values(values, 1, 31))
        {
            return error("monthly date 规则必须包含有效的 monthDays");
        }
    }
    if rule
        .months
        .as_ref()
        .is_some_and(|values| !valid_values(values, 1, 12))
    {
        return error("months 必须是 1-12 的非空整数数组");
    }
    if rule
        .month_days
        .as_ref()
        .is_some_and(|values| !valid_values(values, 1, 31))
    {
        return error("monthDays 必须是 1-31 的非空整数数组");
    }
    Ok(())
}

fn valid_values(values: &[i64], min: i64, max: i64) -> bool {
    !values.is_empty() && values.iter().all(|value| (min..=max).contains(value))
}

/// carrier 校验用原始 JSON：字段「是否存在」与 `null` 语义不同。
fn carrier_input(params: &CreateParams) -> Map<String, Value> {
    let mut input = Map::new();
    if let Some(unit) = params.interval_unit {
        input.insert(
            "intervalUnit".to_string(),
            serde_json::to_value(unit).unwrap_or(Value::Null),
        );
    }
    if let Some(interval) = params.interval {
        input.insert("interval".to_string(), json_number(interval));
    }
    if let Some(delay) = params.relative_delay_minutes {
        input.insert("relativeDelayMinutes".to_string(), json_number(delay));
    }
    if let Some(rule) = &params.schedule_rule {
        input.insert(
            "scheduleRule".to_string(),
            serde_json::to_value(rule).unwrap_or(Value::Null),
        );
    }
    if let Some(recurring) = params.recurring {
        input.insert("recurring".to_string(), Value::Bool(recurring));
    }
    if let Some(max_runs) = params.max_runs {
        input.insert("maxRuns".to_string(), Value::Number(max_runs.into()));
    }
    input
}

fn carrier_input_for_update(params: &UpdateParams) -> Map<String, Value> {
    let mut input = Map::new();
    if let Some(unit) = params.interval_unit {
        input.insert(
            "intervalUnit".to_string(),
            serde_json::to_value(unit).unwrap_or(Value::Null),
        );
    }
    if let Some(interval) = params.interval {
        input.insert("interval".to_string(), json_number(interval));
    }
    // `scheduleRule: null` 也参与互斥判断（显式清除不能与 carrier 混用）。
    if let Some(rule) = &params.schedule_rule {
        input.insert(
            "scheduleRule".to_string(),
            rule.as_ref()
                .map(|rule| serde_json::to_value(rule).unwrap_or(Value::Null))
                .unwrap_or(Value::Null),
        );
    }
    if let Some(recurring) = params.recurring {
        input.insert("recurring".to_string(), Value::Bool(recurring));
    }
    if let Some(Some(max_runs)) = params.max_runs {
        input.insert("maxRuns".to_string(), Value::Number(max_runs.into()));
    }
    input
}

fn json_number(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// `resolveScopeKey`：没有 workspacePath 时返回 None；否则 identity 优先。
fn scope_key(scope: &AutomationScope) -> Option<String> {
    let workspace_path = scope.workspace_path.as_deref()?.trim();
    if workspace_path.is_empty() {
        return None;
    }
    match scope.workspace_identity.as_deref().map(str::trim) {
        Some(identity) if !identity.is_empty() => Some(identity.to_string()),
        _ => Some(workspace_path.to_string()),
    }
}

pub use automation_repo::AutomationScope as Scope;
