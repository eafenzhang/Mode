//! 测试用 Agent 子进程：在 stdin/stdout 上说 Mode Protocol 的最小子集。
//!
//! 只由 `tests/process.rs` 通过 `CARGO_BIN_EXE_agent-process-fixture` 拉起。
//! 模式：
//! - 默认：逐行回 `{id, result:{method}}`，EOF 后退出；
//! - `exit-immediately`：立即退出（用于验证启动即关闭）；
//! - `stubborn`：EOF 后继续存活（用于验证强杀）。

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{json, Value};

/// v4 `conversationSnapshot` 的最小合法形状。
fn minimal_v4_snapshot() -> Value {
    json!({
        "protocolVersion": 1,
        "sessionId": "sess-1",
        "logEpoch": "epoch-1",
        "seq": 1,
        "revision": 0,
        "control": {
            "phase": "draft",
            "sessionEnded": false,
            "canStop": false,
            "stopState": "idle",
            "stopTargetKind": "unknown",
            "activeWorks": [],
            "lastError": null,
            "apiRetry": null
        },
        "availability": {
            "fork": { "allowed": true },
            "compact": { "allowed": true },
            "switchModelConfig": { "allowed": true },
            "setFollowupMode": { "allowed": true },
            "queueEdit": { "allowed": true },
            "sendQueuedNow": { "allowed": true },
            "pauseGoal": { "allowed": true },
            "resumeGoal": { "allowed": true }
        },
        "inputRouting": { "mode": "startNow" },
        "meta": { "title": "", "titleSource": "default" },
        "config": {
            "provider": "zai",
            "model": "glm-4.6",
            "thought": "low",
            "thoughtLevels": [],
            "followupMode": "queue",
            "mode": "build"
        },
        "modelTransition": null,
        "usage": {
            "contextWindow": null,
            "cumulative": {
                "inputTokens": 0,
                "outputTokens": 0,
                "cacheReadTokens": 0,
                "cacheWriteTokens": 0
            }
        },
        "queue": { "items": [], "autoDrain": true },
        "pendingInteractions": [],
        "pendingCommands": [],
        "backgroundWorks": [],
        "goal": null,
        "plan": null,
        "workspaceHookAdmission": null,
        "rows": { "window": [], "totalCount": 0, "firstRowId": null }
    })
}

fn append_log(text: &str) {
    if let Ok(log_path) = std::env::var("AGENT_FIXTURE_FRAME_LOG") {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
        {
            let _ = writeln!(file, "{text}");
        }
    }
}

fn write_notification(stdout: &mut std::io::Stdout, params: Value) {
    let message = json!({ "method": "startup/storageState", "params": params });
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}

fn write_reverse_preferences(stdout: &mut std::io::Stdout, invalid_scope: bool) {
    let scope = if invalid_scope { "bogus" } else { "user-execution" };
    let request = json!({
        "id": "server-1",
        "method": "session/requestRuntimePreferences",
        "params": { "sessionId": "sess-1", "scope": scope }
    });
    let _ = writeln!(stdout, "{request}");
    let _ = stdout.flush();
}

/// `session/read` 与三个 setter 共用的最小合法快照。
fn minimal_snapshot() -> Value {
    json!({
        "protocol": { "name": "Mode Protocol", "version": 1 },
        "session": {
            "sessionId": "sess-1",
            "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
            "sessionKind": "interactive",
            "title": "fixture",
            "mode": "build",
            "status": "idle",
            "createdAt": 1,
            "updatedAt": 2
        },
        "settings": {
            "model": { "available": [] },
            "thoughtLevel": { "enabled": false, "available": [] },
            "mode": { "current": "build" }
        },
        "projection": {
            "sessionId": "sess-1",
            "status": "idle",
            "mode": "build",
            "turnCount": 0,
            "totalTokenCount": 0,
            "contextUsed": 0,
            "contextWindow": 128000,
            "pendingPermissions": [],
            "activeToolCalls": [],
            "backgroundJobs": []
        },
        "runtime": { "eventSeq": 0, "stateRevision": 0, "pendingRequestIds": [] },
        "messages": []
    })
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "exit-immediately" {
        return;
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    if mode == "storage-ready" {
        write_notification(
            &mut stdout,
            json!({
                "schemaVersion": 1, "attemptId": "attempt-1", "sequence": 1,
                "databaseId": "db-1", "databaseKind": "session",
                "phase": "checking", "elapsedMs": 1.0
            }),
        );
        write_notification(
            &mut stdout,
            json!({
                "schemaVersion": 1, "attemptId": "attempt-1", "sequence": 2,
                "databaseId": "db-1", "databaseKind": "session",
                "phase": "ready", "elapsedMs": 2.0
            }),
        );
    }
    if mode == "storage-failed" {
        write_notification(
            &mut stdout,
            json!({
                "schemaVersion": 1, "attemptId": "attempt-1", "sequence": 1,
                "databaseId": "db-1", "databaseKind": "session",
                "phase": "failed", "elapsedMs": 1.0, "errorCode": "sql_failed"
            }),
        );
    }
    if mode == "reverse-runtime-preferences" || mode == "reverse-runtime-preferences-invalid" {
        write_reverse_preferences(&mut stdout, mode.ends_with("invalid"));
    }
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // 反向请求的应答：记下来给测试断言，然后继续等后续请求。
        if method.is_empty()
            && request.get("id").and_then(Value::as_str) == Some("server-1")
            && (request.get("result").is_some() || request.get("error").is_some())
        {
            append_log(&format!("reply\t{request}"));
            continue;
        }
        if mode == "reverse-on-trigger" && method == "test/trigger" {
            write_reverse_preferences(&mut stdout, false);
        }
        eprintln!("handled:{method}");
        let frame_line = format!(
            "frame\t{method}\t{}",
            request.get("params").cloned().unwrap_or(Value::Null)
        );
        eprintln!("{frame_line}");
        append_log(&frame_line);
        if mode == "session-events-stream" && method == "session/subscribe" {
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let event = |event_id: &str, session_id: &str| {
                json!({
                    "eventId": event_id,
                    "sessionId": session_id,
                    "seq": 0,
                    "timestamp": 1,
                    "type": "session.updated",
                    "payload": {}
                })
            };
            let reply = json!({
                "id": id,
                "result": {
                    "sessionId": "sess-1",
                    "eventSeq": 0,
                    "events": [event("ev-1", "sess-1"), event("ev-2", "sess-1")]
                }
            });
            let _ = writeln!(stdout, "{reply}");
            let _ = stdout.flush();
            std::thread::sleep(Duration::from_millis(50));
            // 重复 id（应被丢弃）、新事件（应补 seq）、其它会话（应被过滤）。
            for live in [
                event("ev-2", "sess-1"),
                event("ev-3", "sess-1"),
                event("ev-other", "sess-2"),
            ] {
                let notification = json!({ "method": "session/event", "params": live });
                let _ = writeln!(stdout, "{notification}");
            }
            let _ = stdout.flush();
            continue;
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let reply = if method == "workspace/hooks/trustGrant" {
            match mode.as_str() {
                "error-grant" => json!({
                    "id": id,
                    "error": { "code": -32000, "message": "boom" }
                }),
                "bad-grant" => json!({ "id": id, "result": { "accepted": "yes" } }),
                _ => json!({ "id": id, "result": { "accepted": true } }),
            }
        } else if method == "provider/updateAccountConfig" {
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let revision = params
                .get("revision")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let provider_count = params
                .get("providers")
                .and_then(Value::as_object)
                .map(|providers| providers.len())
                .unwrap_or(0);
            let received = if mode == "bad-provider-revision" {
                "different-revision".to_string()
            } else {
                revision
            };
            json!({
                "id": id,
                "result": {
                    "receivedRevision": received,
                    "providerCount": provider_count,
                    "status": "received"
                }
            })
        } else if method == "session/list" {
            let mut session = json!({
                "sessionId": "sess-1",
                "workspace": {
                    "workspacePath": "/ws/fixture",
                    "workspaceKey": "/ws/fixture"
                },
                "sessionKind": "interactive",
                "title": "fixture",
                "mode": "build",
                "status": "idle",
                "createdAt": 1,
                "updatedAt": 2
            });
            if mode == "bad-session-list" {
                session["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": { "sessions": [session] } })
        } else if method == "session/events" {
            let mut event = json!({
                "eventId": "evt-1",
                "sessionId": "sess-1",
                "seq": 11,
                "timestamp": 1,
                "type": "session.updated",
                "payload": {}
            });
            if mode == "bad-session-events" {
                event["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": { "events": [event] } })
        } else if method == "session/subagents" {
            let mut result = json!({
                "revision": 3,
                "childSessionIds": ["sess-2"],
                "running": [{
                    "childSessionId": "sess-2",
                    "subagentType": "explore",
                    "title": "运行中",
                    "status": "running"
                }],
                "ended": { "total": 0, "items": [] }
            });
            if mode == "bad-session-subagents" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "session/messages" {
            let mut message = json!({
                "info": {
                    "messageId": "msg-1",
                    "sessionId": "sess-1",
                    "role": "user",
                    "time": { "created": 1 },
                    "agent": "mode"
                },
                "parts": [{
                    "partId": "part-1",
                    "sessionId": "sess-1",
                    "messageId": "msg-1",
                    "type": "text",
                    "text": "hi"
                }]
            });
            if mode == "bad-session-messages" {
                message["info"]["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": { "messages": [message] } })
        } else if method == "v4/conversation/subscribe" {
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let reply = json!({
                "id": id,
                "result": {
                    "ack": {
                        "subscriptionId": "sub-1",
                        "mode": "snapshot",
                        "logEpoch": "epoch-1",
                        "openTiming": { "version": 1 }
                    }
                }
            });
            let _ = writeln!(stdout, "{reply}");
            let _ = stdout.flush();
            std::thread::sleep(Duration::from_millis(50));
            let complete = |subscription_id: &str, frame_id: &str| {
                json!({
                    "wireVersion": 3,
                    "kind": "complete",
                    "deliveryKind": "initial",
                    "logicalFrameId": frame_id,
                    "logicalFrameOrdinal": 1,
                    "topic": "conversation/sess-1",
                    "subscriptionId": subscription_id,
                    "frame": {
                        "topic": "conversation/sess-1",
                        "subscriptionId": subscription_id,
                        "fromSeq": 0,
                        "toSeq": 1,
                        "sentAt": 1,
                        "payload": { "kind": "snapshot", "snapshot": minimal_v4_snapshot() }
                    }
                })
            };
            let deltas = json!({
                "wireVersion": 3,
                "kind": "complete",
                "deliveryKind": "online",
                "logicalFrameId": "f3",
                "logicalFrameOrdinal": 3,
                "topic": "conversation/sess-1",
                "subscriptionId": "sub-1",
                "frame": {
                    "topic": "conversation/sess-1",
                    "subscriptionId": "sub-1",
                    "fromSeq": 1,
                    "toSeq": 2,
                    "sentAt": 2,
                    "payload": {
                        "kind": "deltas",
                        "deltas": [{ "op": "row.delta", "rowId": 1, "path": "text", "append": "x" }]
                    }
                }
            });
            let fragment = json!({
                "wireVersion": 3,
                "kind": "fragment",
                "deliveryKind": "online",
                "logicalFrameId": "f2",
                "logicalFrameOrdinal": 2,
                "topic": "conversation/sess-1",
                "subscriptionId": "sub-1",
                "fragmentIndex": 0,
                "fragmentCount": 1,
                "logicalBytes": 10,
                "checksum": { "algorithm": "crc32", "value": "00000000" },
                "dataBase64": "e30="
            });
            for params in [
                complete("sub-1", "f1"),
                complete("sub-other", "f9"),
                deltas,
                fragment,
            ] {
                let notification = json!({ "method": "v4/conversation/frame", "params": params });
                let _ = writeln!(stdout, "{notification}");
            }
            let _ = stdout.flush();
            continue;
        } else if method == "v4/command" {
            let command_id = request
                .get("params")
                .and_then(|params| params.get("commandId"))
                .and_then(Value::as_str)
                .unwrap_or("cmd-1")
                .to_string();
            let rejected = mode == "v4-command-rejected";
            json!({
                "id": id,
                "result": if rejected {
                    json!({
                        "commandId": command_id,
                        "status": "rejected",
                        "reasonCode": "fault.command.rejected",
                        "message": "denied",
                        "revisionAtDecision": 5
                    })
                } else {
                    json!({
                        "commandId": command_id,
                        "status": "accepted",
                        "revisionAtDecision": 5
                    })
                }
            })
        } else if method == "v4/conversation/rowsRange" {
            let mut result = json!({
                "rows": [{
                    "rowId": 1,
                    "turnId": "turn-1",
                    "createdAt": 1,
                    "createdAtSeq": 1,
                    "kind": "turnHeader",
                    "origin": "userInput",
                    "state": "running",
                    "startedAt": 1
                }],
                "atSeq": 3,
                "atRevision": 2,
                "atLogEpoch": "epoch-1",
                "hasMore": true
            });
            if mode == "bad-v4-rows-range" {
                // 结果 schema 非严格：未知键被 strip，只有类型不符才会被拒绝。
                result["hasMore"] = json!("yes");
            }
            json!({ "id": id, "result": result })
        } else if method == "v4/conversation/plans" {
            let mut result = json!({
                "plans": [{
                    "rowId": 6,
                    "turnId": "turn-1",
                    "createdAt": 1,
                    "createdAtSeq": 6,
                    "kind": "toolCall",
                    "toolCallId": "call-1",
                    "toolName": "ExitPlanMode",
                    "status": "success",
                    "inputText": "{}"
                }],
                "atSeq": 3,
                "atLogEpoch": "epoch-1"
            });
            if mode == "bad-v4-plans" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "v4/conversation/workflowRunEvents" {
            let mut result = json!({
                "events": [{
                    "sequence": 7,
                    "type": "phase.started",
                    "payload": { "phase": "build" }
                }],
                "hasMore": true
            });
            if mode == "bad-v4-workflow-run-events" {
                result["hasMore"] = json!("yes");
            }
            json!({ "id": id, "result": result })
        } else if method == "v4/conversation/workflowRuns" {
            let mut result = json!({
                "runs": [{
                    "runId": "run-1",
                    "toolCallId": "call-1",
                    "label": "发布",
                    "updatedAt": 1700000000000u64,
                    "status": "stopped",
                    "stopReason": "superseded",
                    "supersededBy": "run-2",
                    "failureCode": "Interrupted",
                    "failureMessage": "用户中止",
                    "resumable": true
                }]
            });
            if mode == "bad-v4-workflow-runs" {
                result["runs"][0]["status"] = json!("archived");
            }
            json!({ "id": id, "result": result })
        } else if method == "plugins/overview" {
            let mut result = json!({
                "marketplaces": [{
                    "id": "official",
                    "name": "官方",
                    "source": {},
                    "pluginCount": 1,
                    "isOfficial": true,
                    "featured": ["demo"]
                }],
                "availablePlugins": [{
                    "id": "demo-plugin",
                    "name": "demo",
                    "marketplace": "official",
                    "installed": false,
                    "componentTypes": ["skill"],
                    "listing": { "displayName": "Demo", "requiresPaidPlan": false }
                }],
                "installedPlugins": [{
                    "id": "demo-plugin",
                    "name": "demo",
                    "marketplace": "official",
                    "enabled": true,
                    "scope": "workspace",
                    "updateStatus": "none"
                }],
                "restorableBuiltins": [],
                "diagnostics": [],
                "officialMarketplaceEnabled": true,
                "capability": { "supported": true }
            });
            if mode == "bad-plugins-overview" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "plugins/list" {
            let mut result = json!({
                "plugins": [{
                    "id": "demo-plugin",
                    "name": "demo",
                    "enabled": true,
                    "source": "marketplace",
                    "marketplace": "official",
                    "skillRootCount": 1,
                    "commandRootCount": 0,
                    "mcpServerNames": [],
                    "rootPath": "/home/u/.mode/plugins/demo",
                    "components": [{ "kind": "skill", "items": [{ "name": "run" }] }],
                    "hookDetails": [{
                        "event": "PreToolUse",
                        "type": "command",
                        "command": "node",
                        "shell": true,
                        "sourcePath": "/home/u/.mode/plugins/demo/hooks.json",
                        "runnable": true
                    }]
                }],
                "diagnostics": [{ "code": "warn", "message": "演示诊断", "severity": "warning" }]
            });
            if mode == "bad-plugins-list" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "plugins/describe" {
            let mut result = json!({
                "components": [{
                    "kind": "skill",
                    "items": [{ "name": "run", "description": "运行" }]
                }],
                "metadata": { "author": "demo", "version": "1.0.0" }
            });
            if mode == "bad-plugins-describe" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "workflows/updateMeta" {
            let mut result = json!({ "ok": true, "path": "/ws/demo/.mode/workflows/demo.md" });
            if mode == "bad-workflow-update-meta" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "workflows/delete" {
            let mut result = json!({ "ok": true, "path": "/ws/demo/.mode/workflows/demo.md" });
            if mode == "bad-workflow-delete" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "workflows/move" {
            let mut result = json!({
                "ok": true,
                "from": "/home/u/.mode/workflows/demo.md",
                "to": "/ws/demo/.mode/workflows/demo.md"
            });
            if mode == "bad-workflow-move" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "workflows/list" {
            let mut result = json!({
                "workflows": [{
                    "name": "demo",
                    "description": "演示工作流",
                    "scope": "project",
                    "path": "/ws/demo/.mode/workflows/demo.md"
                }],
                "invalid": [],
                "dir": "/ws/demo/.mode/workflows"
            });
            if mode == "bad-workflows-list" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "workflows/get" {
            let mut result = if mode == "workflow-get-failure" {
                json!({ "ok": false, "reason": "not_found" })
            } else {
                json!({
                    "ok": true,
                    "name": "demo",
                    "path": "/ws/demo/.mode/workflows/demo.md",
                    "scope": "project",
                    "meta": { "description": "演示工作流" },
                    "script": "print(1)"
                })
            };
            if mode == "bad-workflow-get" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "workflows/runs" {
            let mut result = json!({
                "runs": [{
                    "runId": "run-1",
                    "name": "demo",
                    "status": "completed",
                    "createdAt": 1,
                    "updatedAt": 2,
                    "spentTokens": 10,
                    "cwd": "/ws/demo",
                    "artifacts": [{
                        "id": "art-1",
                        "kind": "markdown",
                        "title": "报告",
                        "version": 1,
                        "contentType": "text/markdown"
                    }]
                }],
                "truncated": true
            });
            if mode == "bad-workflow-runs" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "mcp/list" {
            let mut result = json!({
                "statuses": {
                    "mcp-1": {
                        "status": "connected",
                        "transport": "stdio",
                        "toolCount": 3,
                        "updatedAt": "t1"
                    }
                }
            });
            if mode == "bad-mcp-list" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "workspace/readPresentation" {
            let mut result = json!({
                "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
                "mode": "build",
                "slashCommands": [{ "name": "/help", "description": "帮助", "source": "builtin" }]
            });
            if mode == "bad-workspace-presentation" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "process/childProcesses" {
            let mut result = json!({
                "processes": [{
                    "pid": 1234,
                    "serverName": "mcp-1",
                    "mcpSource": "custom"
                }]
            });
            if mode == "bad-child-processes" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "session/send" {
            let accepted = mode != "rejected-session-send";
            json!({
                "id": id,
                "result": {
                    "accepted": accepted,
                    "sessionId": "sess-1",
                    "stateRevision": 3
                }
            })
        } else if method == "session/create" {
            let mut snapshot = minimal_snapshot();
            if mode == "bad-session-create" {
                snapshot["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": snapshot })
        } else if method == "session/read" {
            let mut snapshot = minimal_snapshot();
            if mode == "bad-session-read" {
                snapshot["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": snapshot })
        } else if method == "session/setModel"
            || method == "session/setThoughtLevel"
            || method == "session/setMode"
        {
            let mut snapshot = minimal_snapshot();
            if mode == "bad-session-setter" {
                snapshot["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": snapshot })
        } else if method == "session/close" {
            let mut result = json!({ "closed": true });
            if mode == "bad-session-close" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "session/debug" {
            let mut snapshot = json!({
                "sessionId": "sess-1",
                "rounds": [],
                "networkEntries": [],
                "cache": null
            });
            if mode == "bad-session-debug" {
                snapshot["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": snapshot })
        } else {
            json!({ "id": id, "result": { "method": method } })
        };
        if mode == "slow-reply" {
            // 让请求保持「在飞」，用于空闲回收不得中途回收的用例。
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        if writeln!(stdout, "{reply}").is_err() {
            break;
        }
        if stdout.flush().is_err() {
            break;
        }
    }

    if mode == "stubborn" {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}
