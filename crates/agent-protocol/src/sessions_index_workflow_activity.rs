//! sessions-index 的工作流运行摘要（`deriveSessionWorkflowActivity` 的 Rust 版）。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/sessions-index-workflow-activity.ts`。
//!
//! 为什么存在：侧栏要在会话标题下画一条「Workflow 图标 + 迷你轨道灯 + 当前 phase 名」，
//! 但不能为此订阅整个 `workflowRuns`（那是 conversation 权威投影，按 run 进度高频变化）。
//! 这里把同一 snapshot 的 `workflowRuns` + `backgroundWorks` 派生成**有界、只装侧栏
//! 真正读的字段**的摘要。纯函数、无时钟——「已结束的行何时折叠」由渲染端决定。
//!
//! 三条容易写错的地方（各自对应 TS 注释里点明的坑）：
//!
//! 1. **站点灯要区分「控制流到达」与「还有节点在跑」**。控制流只记得最后一个标记，但
//!    并行的 A 在 B 之后仍可能在干活——所以除了 `current_phase`，还要看「出生在这一站
//!    且此刻还在跑」的节点。
//! 2. **`dispatched` / `waiting` 不算在跑**。前者是「会话就绪、首个请求尚未准入」的
//!    短暂相位，后者在等槽位或在退避；真正发出去了由 `executing` 说。
//! 3. **裁表之后下标空间跟着变窄**。指向被裁掉站点的 `alongside` 引用要一起丢掉，
//!    剩下的才建键。

use serde_json::Number;

use crate::v4_sessions_index::{
    SessionWorkflowActivity, SessionWorkflowPhaseStatus, SessionWorkflowPhaseSummary,
    SessionWorkflowRunSummary, SESSION_WORKFLOW_ACTIVITY_MAX_RUNS, SESSION_WORKFLOW_MAX_PHASES,
    SESSION_WORKFLOW_MAX_PHASE_NAME_LENGTH,
};
use crate::v4_snapshot::{BackgroundWorkKind, BackgroundWorkSummary};
use crate::v4_workflow_runs::{
    WorkflowRunActorStatus, WorkflowRunNode, WorkflowRunNodePhase, WorkflowRunState,
    WorkflowRunStatus, WorkflowRunsState,
};

/// 在跑（`pending` / `running`）的 run 排在前面。
pub fn is_session_workflow_run_live(status: WorkflowRunStatus) -> bool {
    matches!(
        status,
        WorkflowRunStatus::Pending | WorkflowRunStatus::Running
    )
}

/// 「这个节点此刻真的在跑」：与 `packages/ui` 的 `statusOfRunNode` 的 running 臂一致。
///
/// `dispatched` / `waiting` **不算**（见模块注释第 2 条）。
fn is_run_node_running(node: &WorkflowRunNode) -> bool {
    matches!(
        node.phase,
        WorkflowRunNodePhase::Executing
            | WorkflowRunNodePhase::Repairing
            | WorkflowRunNodePhase::Nudged
    )
}

/// 站名 → 实例出生巢的关联规则：精确匹配，站名**恰好顶到上界**时才按前缀兜底。
///
/// 前缀只在截断真的发生过时才开，否则「计划」会误认「计划修复」。
fn phase_name_matches(station_name: &str, stamp: &str) -> bool {
    station_name == stamp
        || (station_name.chars().count() >= SESSION_WORKFLOW_MAX_PHASE_NAME_LENGTH
            && stamp.starts_with(station_name))
}

fn derive_phases(run: &WorkflowRunState) -> Vec<SessionWorkflowPhaseSummary> {
    let entered: Vec<&str> = run
        .phases
        .as_ref()
        .map(|phases| phases.iter().map(|phase| phase.name.as_str()).collect())
        .unwrap_or_default();
    let current = run.current_phase.as_deref();
    let live = is_session_workflow_run_live(run.status);

    // 「同时在跑」的下标说的是**声明表**里的位置，所以只有走声明表这条路时才有意义。
    let (names, alongside): (Vec<String>, Option<&Vec<Vec<u64>>>) = match &run.phase_names {
        Some(declared) if !declared.is_empty() => (declared.clone(), run.phase_alongside.as_ref()),
        _ => {
            let mut names: Vec<String> = run
                .phases
                .as_ref()
                .map(|phases| phases.iter().map(|phase| phase.name.clone()).collect())
                .unwrap_or_default();
            if let Some(current) = current {
                if !entered.contains(&current) {
                    names.push(current.to_string());
                }
            }
            (names, None)
        }
    };

    // 在跑的节点的出生巢。run 不在跑时整张表为空：终态 run 里没有人还在干活。
    let burning: Vec<&str> = if live {
        run.nodes
            .iter()
            .filter(|node| is_run_node_running(node))
            .filter_map(|node| node.phase_name.as_deref())
            .collect()
    } else {
        Vec::new()
    };

    let emitted = names.len().min(SESSION_WORKFLOW_MAX_PHASES);
    names
        .iter()
        .take(SESSION_WORKFLOW_MAX_PHASES)
        .enumerate()
        .map(|(index, name)| {
            let is_current = current == Some(name.as_str());
            let was_entered = entered.contains(&name.as_str()) || is_current;
            let burning_here = burning.iter().any(|stamp| phase_name_matches(name, stamp));
            let status = if live {
                if is_current || burning_here {
                    SessionWorkflowPhaseStatus::Running
                } else if was_entered {
                    SessionWorkflowPhaseStatus::Done
                } else {
                    SessionWorkflowPhaseStatus::Pending
                }
            } else if run.status == WorkflowRunStatus::Completed {
                if was_entered {
                    SessionWorkflowPhaseStatus::Done
                } else {
                    SessionWorkflowPhaseStatus::Pending
                }
            } else if run.status == WorkflowRunStatus::Errored {
                if is_current {
                    SessionWorkflowPhaseStatus::Failed
                } else if was_entered {
                    SessionWorkflowPhaseStatus::Done
                } else {
                    SessionWorkflowPhaseStatus::Pending
                }
            } else {
                // stopped：控制流停在当前 phase，它没有完成也没有失败——留空心灯。
                //
                // **is_current 必须显式判**：它大概率也在 entered 里（进过才有当前），
                // 漏掉这一支会让「停下的站」被画成已完成——与「空心灯」的语义正好相反。
                if is_current {
                    SessionWorkflowPhaseStatus::Pending
                } else if was_entered {
                    SessionWorkflowPhaseStatus::Done
                } else {
                    SessionWorkflowPhaseStatus::Pending
                }
            };
            // 裁表之后下标空间跟着变窄：指向被裁掉站点的引用一起丢掉，剩下空的就不建键。
            let beside: Vec<u64> = alongside
                .and_then(|table| table.get(index))
                .map(|others| {
                    others
                        .iter()
                        .copied()
                        .filter(|other| {
                            let other = *other as usize;
                            other < emitted && other != index
                        })
                        .collect()
                })
                .unwrap_or_default();
            SessionWorkflowPhaseSummary {
                name: name.clone(),
                status,
                alongside: (!beside.is_empty()).then_some(beside),
            }
        })
        .collect()
}

fn summarize_run(
    run: &WorkflowRunState,
    work: Option<&BackgroundWorkSummary>,
) -> SessionWorkflowRunSummary {
    let name = work
        .map(|work| work.title.trim())
        .filter(|title| !title.is_empty())
        .map(str::to_string);
    SessionWorkflowRunSummary {
        run_id: run.run_id.clone(),
        tool_call_id: run.tool_call_id.clone(),
        name,
        status: run.status,
        stop_reason: run.stop_reason,
        started_at: work.map(|work| work.started_at.clone()),
        phases: derive_phases(run),
        current_phase: run.current_phase.clone(),
        agents_working: run
            .actors
            .iter()
            .filter(|actor| actor.status == WorkflowRunActorStatus::Running)
            .count() as u64,
    }
}

/// 从同一 snapshot 的 `workflowRuns` + `backgroundWorks` 派生工作流运行摘要。
///
/// 在跑的 run 按 `runs[]` 序（= 启动序）在前；其后是已结束的，按后台工作的 `endedAt`
/// 倒序、无 `endedAt` 时按 `runs[]` 倒序（越晚建的越新）。总数裁到
/// [`SESSION_WORKFLOW_ACTIVITY_MAX_RUNS`]。
///
/// 会话一个 run 都没有 → `None`（键整个缺席，旧 CLI 因此什么都不变）。
pub fn derive_session_workflow_activity(
    workflow_runs: Option<&WorkflowRunsState>,
    background_works: &[BackgroundWorkSummary],
) -> Option<SessionWorkflowActivity> {
    let runs = workflow_runs
        .map(|state| state.runs.as_slice())
        .unwrap_or(&[]);
    if runs.is_empty() {
        return None;
    }
    // 只有 kind === workflow 的后台工作才对应 run。
    let work_by_run_id: Vec<(&str, &BackgroundWorkSummary)> = background_works
        .iter()
        .filter(|work| work.kind == BackgroundWorkKind::Workflow)
        .map(|work| (work.work_id.as_str(), work))
        .collect();

    let mut live: Vec<SessionWorkflowRunSummary> = Vec::new();
    let mut settled: Vec<(SessionWorkflowRunSummary, i64, usize)> = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        let work = work_by_run_id
            .iter()
            .find(|(work_id, _)| *work_id == run.run_id)
            .map(|(_, work)| *work);
        let summary = summarize_run(run, work);
        if is_session_workflow_run_live(run.status) {
            live.push(summary);
        } else {
            let ended_at = work
                .and_then(|work| work.ended_at.as_ref())
                .and_then(Number::as_i64)
                .unwrap_or(0);
            settled.push((summary, ended_at, index));
        }
    }
    // endedAt 倒序；相同则按 runs[] 倒序（越晚建的越新）。
    settled.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.2.cmp(&left.2)));

    let mut combined = live;
    combined.extend(settled.into_iter().map(|(summary, _, _)| summary));
    combined.truncate(SESSION_WORKFLOW_ACTIVITY_MAX_RUNS);
    Some(SessionWorkflowActivity { runs: combined })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v4_workflow_runs::{WorkflowRunPhase, WorkflowRunUsage};
    use serde_json::json;

    fn run(run_id: &str, status: WorkflowRunStatus) -> WorkflowRunState {
        WorkflowRunState {
            run_id: run_id.to_string(),
            tool_call_id: None,
            status,
            stop_reason: None,
            resumed_from: None,
            superseded_by: None,
            usage: WorkflowRunUsage {
                spent_tokens: 0,
                nodes_used: 0,
                nodes_unlisted: None,
                nodes_unlisted_settled: None,
            },
            error: None,
            resumable: None,
            result_preview: None,
            actors: Vec::new(),
            nodes: Vec::new(),
            reports: None,
            pending_questions: None,
            concurrency: None,
            concurrency_ceiling: None,
            subagent_model: None,
            artifacts: None,
            phases: None,
            current_phase: None,
            phase_names: None,
            phase_alongside: None,
            unlisted_by_phase: None,
            truncated: None,
            last_event_sequence: 0,
        }
    }

    fn phases(names: &[&str]) -> Option<Vec<WorkflowRunPhase>> {
        Some(
            names
                .iter()
                .map(|name| WorkflowRunPhase {
                    name: (*name).to_string(),
                    rounds: 1,
                })
                .collect(),
        )
    }

    #[test]
    fn no_runs_yields_none_so_the_key_stays_absent() {
        assert_eq!(derive_session_workflow_activity(None, &[]), None);
        let empty = WorkflowRunsState {
            revision: 1,
            runs: Vec::new(),
        };
        assert_eq!(derive_session_workflow_activity(Some(&empty), &[]), None);
    }

    #[test]
    fn live_run_phases_mark_current_as_running_and_entered_as_done() {
        let mut state = run("r1", WorkflowRunStatus::Running);
        // phases = 已进入的站（plan、build）；声明表给出全站清单（含尚未进入的 verify）。
        state.phases = phases(&["plan", "build"]);
        state.phase_names = Some(vec![
            "plan".to_string(),
            "build".to_string(),
            "verify".to_string(),
        ]);
        state.current_phase = Some("build".to_string());
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![state],
            }),
            &[],
        )
        .expect("activity");

        let phases = &activity.runs[0].phases;
        assert_eq!(phases.len(), 3);
        // plan 进过但当前不是它 → done（控制流已走过）
        assert_eq!(phases[0].status, SessionWorkflowPhaseStatus::Done);
        // build 是 current → running
        assert_eq!(phases[1].status, SessionWorkflowPhaseStatus::Running);
        // verify 没进过 → pending
        assert_eq!(phases[2].status, SessionWorkflowPhaseStatus::Pending);
    }

    #[test]
    fn a_parallel_node_still_burning_keeps_its_station_lit() {
        // 控制流已经走到 build，但 plan 站出生的节点还在跑（并行）：
        // 只按 current_phase 判断会让 plan 灯提前熄灭。
        let mut state = run("r1", WorkflowRunStatus::Running);
        state.phases = phases(&["plan", "build"]);
        state.current_phase = Some("build".to_string());
        state.nodes.push(WorkflowRunNode {
            site_id: "site-1".to_string(),
            ordinal: 0,
            kind: None,
            phase: WorkflowRunNodePhase::Executing,
            outcome: None,
            cached: None,
            actor_site_id: None,
            actor_ordinal: None,
            phase_name: Some("plan".to_string()),
            instructions_head: None,
            turn: None,
            tool_calls: None,
            last_tool: None,
        });
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![state],
            }),
            &[],
        )
        .expect("activity");

        assert_eq!(
            activity.runs[0].phases[0].status,
            SessionWorkflowPhaseStatus::Running,
            "并行的 A 在 B 之后仍在干活，plan 灯不能熄"
        );
        assert_eq!(
            activity.runs[0].phases[1].status,
            SessionWorkflowPhaseStatus::Running
        );
    }

    #[test]
    fn dispatched_and_waiting_nodes_do_not_count_as_running() {
        for phase in [
            WorkflowRunNodePhase::Dispatched,
            WorkflowRunNodePhase::Waiting,
            WorkflowRunNodePhase::Queued,
            WorkflowRunNodePhase::Settled,
        ] {
            let mut state = run("r1", WorkflowRunStatus::Running);
            state.phases = phases(&["plan", "build"]);
            state.current_phase = Some("build".to_string());
            state.nodes.push(WorkflowRunNode {
                site_id: "site-1".to_string(),
                ordinal: 0,
                kind: None,
                phase,
                outcome: None,
                cached: None,
                actor_site_id: None,
                actor_ordinal: None,
                phase_name: Some("plan".to_string()),
                instructions_head: None,
                turn: None,
                tool_calls: None,
                last_tool: None,
            });
            let activity = derive_session_workflow_activity(
                Some(&WorkflowRunsState {
                    revision: 1,
                    runs: vec![state],
                }),
                &[],
            )
            .expect("activity");
            assert_eq!(
                activity.runs[0].phases[0].status,
                SessionWorkflowPhaseStatus::Done,
                "{phase:?} 不算在跑"
            );
        }
    }

    #[test]
    fn errored_run_marks_only_the_current_phase_failed() {
        let mut state = run("r1", WorkflowRunStatus::Errored);
        state.phases = phases(&["plan", "build"]);
        state.phase_names = Some(vec![
            "plan".to_string(),
            "build".to_string(),
            "verify".to_string(),
        ]);
        state.current_phase = Some("build".to_string());
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![state],
            }),
            &[],
        )
        .expect("activity");

        let phases = &activity.runs[0].phases;
        assert_eq!(phases[0].status, SessionWorkflowPhaseStatus::Done);
        assert_eq!(phases[1].status, SessionWorkflowPhaseStatus::Failed);
        assert_eq!(
            phases[2].status,
            SessionWorkflowPhaseStatus::Pending,
            "没进过的站不因整 run 失败而被标记"
        );
    }

    #[test]
    fn stopped_run_leaves_the_current_station_pending_not_failed() {
        let mut state = run("r1", WorkflowRunStatus::Stopped);
        state.phases = phases(&["plan", "build"]);
        state.phase_names = Some(vec!["plan".to_string(), "build".to_string()]);
        state.current_phase = Some("build".to_string());
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![state],
            }),
            &[],
        )
        .expect("activity");
        assert_eq!(
            activity.runs[0].phases[1].status,
            SessionWorkflowPhaseStatus::Pending,
            "停下的站既没完成也没失败"
        );
    }

    #[test]
    fn declared_phase_names_win_and_carry_alongside() {
        let mut state = run("r1", WorkflowRunStatus::Running);
        state.phases = phases(&["plan"]);
        state.phase_names = Some(vec!["plan".to_string(), "build".to_string()]);
        state.phase_alongside = Some(vec![vec![], vec![0]]);
        state.current_phase = Some("build".to_string());
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![state],
            }),
            &[],
        )
        .expect("activity");

        let phases = &activity.runs[0].phases;
        assert_eq!(phases.len(), 2, "声明表优先于已进入表");
        assert_eq!(phases[1].alongside, Some(vec![0]));
    }

    #[test]
    fn alongside_indices_out_of_range_are_dropped() {
        let mut state = run("r1", WorkflowRunStatus::Running);
        state.phase_names = Some(vec!["a".to_string(), "b".to_string()]);
        // 指向 9（不存在）与自身（1）都要丢掉。
        state.phase_alongside = Some(vec![vec![], vec![9, 1]]);
        state.current_phase = Some("a".to_string());
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![state],
            }),
            &[],
        )
        .expect("activity");
        assert_eq!(
            activity.runs[0].phases[1].alongside, None,
            "越界与自身的引用都要丢"
        );
    }

    #[test]
    fn live_runs_come_first_then_settled_by_ended_at_desc() {
        let mut live = run("r-live", WorkflowRunStatus::Running);
        live.current_phase = Some("x".to_string());
        let older = run("r-old", WorkflowRunStatus::Completed);
        let newer = run("r-new", WorkflowRunStatus::Completed);

        let works = vec![
            BackgroundWorkSummary {
                work_id: "r-old".to_string(),
                kind: BackgroundWorkKind::Workflow,
                title: "旧".to_string(),
                status: crate::v4_snapshot::BackgroundWorkStatus::ResultPending,
                started_at: Number::from(1u64),
                ended_at: Some(Number::from(100u64)),
                cancellable: None,
                blocked: None,
                anchor_row_id: None,
                child_session_id: None,
            },
            BackgroundWorkSummary {
                work_id: "r-new".to_string(),
                kind: BackgroundWorkKind::Workflow,
                title: "新".to_string(),
                status: crate::v4_snapshot::BackgroundWorkStatus::ResultPending,
                started_at: Number::from(2u64),
                ended_at: Some(Number::from(200u64)),
                cancellable: None,
                blocked: None,
                anchor_row_id: None,
                child_session_id: None,
            },
        ];
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![older, live, newer],
            }),
            &works,
        )
        .expect("activity");

        let ids: Vec<&str> = activity
            .runs
            .iter()
            .map(|run| run.run_id.as_str())
            .collect();
        assert_eq!(ids, vec!["r-live", "r-new", "r-old"]);
    }

    #[test]
    fn non_workflow_background_works_are_ignored() {
        let state = run("r1", WorkflowRunStatus::Completed);
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![state],
            }),
            &[BackgroundWorkSummary {
                work_id: "r1".to_string(),
                kind: BackgroundWorkKind::Bash,
                title: "不该被当成 name".to_string(),
                status: crate::v4_snapshot::BackgroundWorkStatus::ResultPending,
                started_at: Number::from(1u64),
                ended_at: None,
                cancellable: None,
                blocked: None,
                anchor_row_id: None,
                child_session_id: None,
            }],
        )
        .expect("activity");
        assert_eq!(
            activity.runs[0].name, None,
            "只有 kind=workflow 的后台工作才对应 run"
        );
        assert_eq!(activity.runs[0].started_at, None);
    }

    #[test]
    fn runs_are_capped_and_phases_truncated_consistently() {
        let runs: Vec<WorkflowRunState> = (0..6)
            .map(|index| run(&format!("r{index}"), WorkflowRunStatus::Running))
            .collect();
        let activity =
            derive_session_workflow_activity(Some(&WorkflowRunsState { revision: 1, runs }), &[])
                .expect("activity");
        assert_eq!(
            activity.runs.len(),
            SESSION_WORKFLOW_ACTIVITY_MAX_RUNS,
            "总数裁到 4"
        );
    }

    #[test]
    fn long_station_name_falls_back_to_prefix_match_only_at_the_cap() {
        let long = "a".repeat(SESSION_WORKFLOW_MAX_PHASE_NAME_LENGTH);
        assert!(phase_name_matches(&long, &format!("{long}-suffix")));
        // 未顶到上界的站名不允许前缀匹配，否则「计划」会误认「计划修复」。
        assert!(!phase_name_matches("计划", "计划修复"));
        assert!(phase_name_matches("计划", "计划"));
    }

    #[test]
    fn agents_working_counts_only_running_actors() {
        let mut state = run("r1", WorkflowRunStatus::Running);
        state.current_phase = Some("x".to_string());
        state.actors = vec![
            crate::v4_workflow_runs::WorkflowRunActor {
                site_id: "a".to_string(),
                ordinal: 0,
                name: None,
                session_id: None,
                status: WorkflowRunActorStatus::Running,
                phase_name: None,
            },
            crate::v4_workflow_runs::WorkflowRunActor {
                site_id: "b".to_string(),
                ordinal: 1,
                name: None,
                session_id: None,
                status: WorkflowRunActorStatus::Completed,
                phase_name: None,
            },
        ];
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![state],
            }),
            &[],
        )
        .expect("activity");
        assert_eq!(activity.runs[0].agents_working, 1);
    }

    #[test]
    fn json_shape_matches_the_wire_schema() {
        let mut state = run("r1", WorkflowRunStatus::Running);
        state.current_phase = Some("build".to_string());
        state.phases = phases(&["build"]);
        let activity = derive_session_workflow_activity(
            Some(&WorkflowRunsState {
                revision: 1,
                runs: vec![state],
            }),
            &[],
        )
        .expect("activity");
        let encoded = serde_json::to_value(&activity).expect("serialize");
        assert_eq!(encoded["runs"][0]["currentPhase"], json!("build"));
        assert_eq!(encoded["runs"][0]["phases"][0]["status"], json!("running"));
        assert_eq!(encoded["runs"][0]["agentsWorking"], json!(0));
        // 缺席的可选键不出现（与 zod 的 optional 语义一致）。
        assert!(encoded["runs"][0].get("name").is_none());
    }
}
