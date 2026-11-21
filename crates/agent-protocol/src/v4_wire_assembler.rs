//! sessions-index wire 分片装配（真源 `packages/shared/src/mode-protocol-v4/
//! wire-assembler.ts` 的 `TopicWireFrameAssembler`，按当前唯一宿主消费面收敛到
//! `SessionsIndexTopicFrame`）。
//!
//! 语义要点：
//! - 每 `(topic, subscriptionId)` 只允许一个进行中的 logical frame；ordinal 单调，
//!   旧 ordinal 直接忽略，同 ordinal 不同 id 记 `OrdinalConflict`。
//! - 更高 ordinal 的权威 `recovery` 可无 fault 取代残缺旧帧；普通帧取代时对旧帧
//!   记 `Superseded`。
//! - tombstone（settled）保留每个 route 的已收 ordinal，防止旧分片回放复活。
//! - 分片校验上限与 TS 一致：1 MiB 物理帧 / 16 MiB logical / 1024 片 /
//!   32 并发 / 32 MiB 暂存 / 30s 超时；crc32 校验、严格 UTF-8、JSON 与 schema 校验。
//! - envelope 体积用类型化 JSON 序列化近似 `measureTopicNotificationEnvelopeBytes`
//!   （宿主侧预算守卫，不参与协议判定）。

use std::collections::HashMap;

use base64::Engine as _;
use serde_json::Value;

use crate::v4_sessions_index::{SessionsIndexTopicFrame, SessionsIndexWireFrame};
use crate::v4_transport::WireDeliveryKind;

pub const ASSEMBLY_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const ASSEMBLY_MAX_FRAGMENTS: u64 = 1024;
pub const ASSEMBLY_MAX_CONCURRENT: usize = 32;
pub const ASSEMBLY_MAX_STAGED_BYTES: usize = 32 * 1024 * 1024;
pub const ASSEMBLY_TIMEOUT_MS: i64 = 30_000;
pub const ASSEMBLY_MAX_PHYSICAL_BYTES: usize = 1024 * 1024;

pub const FAULT_METADATA_MISMATCH: &str = "proto.frameAssemblyMetadataMismatch";
pub const FAULT_ORDINAL_CONFLICT: &str = "proto.frameAssemblyOrdinalConflict";
pub const FAULT_SUPERSEDED: &str = "proto.frameAssemblySuperseded";
pub const FAULT_ENVELOPE_TOO_LARGE: &str = "proto.frameEnvelopeTooLarge";
pub const FAULT_ASSEMBLY_TOO_LARGE: &str = "proto.frameAssemblyTooLarge";
pub const FAULT_INVALID_PAYLOAD: &str = "proto.frameAssemblyInvalidPayload";
pub const FAULT_FRAGMENT_COUNT_EXCEEDED: &str = "proto.frameFragmentCountExceeded";
pub const FAULT_INVALID_BASE64: &str = "proto.frameAssemblyInvalidBase64";
pub const FAULT_CONCURRENT_LIMIT: &str = "proto.frameAssemblyConcurrentLimit";
pub const FAULT_BUDGET_EXCEEDED: &str = "proto.frameAssemblyBudgetExceeded";
pub const FAULT_FRAGMENT_CONFLICT: &str = "proto.frameAssemblyFragmentConflict";
pub const FAULT_LENGTH_MISMATCH: &str = "proto.frameAssemblyLengthMismatch";
pub const FAULT_CHECKSUM_MISMATCH: &str = "proto.frameAssemblyChecksumMismatch";
pub const FAULT_INVALID_UTF8: &str = "proto.frameAssemblyInvalidUtf8";
pub const FAULT_INVALID_JSON: &str = "proto.frameAssemblyInvalidJson";
pub const FAULT_TIMED_OUT: &str = "proto.frameAssemblyTimedOut";

/// 装配故障（owned typed fault：宿主据此把 route fail-close）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIndexAssemblyFault {
    pub delivery_kind: WireDeliveryKind,
    pub reason_code: &'static str,
    pub logical_frame_id: String,
    pub logical_frame_ordinal: u64,
    pub topic: String,
    pub subscription_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionIndexAssemblyEvent {
    Complete {
        frame: SessionsIndexTopicFrame,
        delivery_kind: WireDeliveryKind,
    },
    Fault(SessionIndexAssemblyFault),
}

#[derive(Clone)]
struct WireMeta {
    delivery_kind: WireDeliveryKind,
    logical_frame_id: String,
    logical_frame_ordinal: u64,
    topic: String,
    subscription_id: String,
}

impl WireMeta {
    fn from_wire(wire: &SessionsIndexWireFrame) -> Self {
        match wire {
            SessionsIndexWireFrame::Complete(frame) => Self {
                delivery_kind: frame.delivery_kind,
                logical_frame_id: frame.logical_frame_id.clone(),
                logical_frame_ordinal: frame.logical_frame_ordinal,
                topic: frame.topic.clone(),
                subscription_id: frame.subscription_id.clone(),
            },
            SessionsIndexWireFrame::Fragment(frame) => Self {
                delivery_kind: frame.delivery_kind,
                logical_frame_id: frame.logical_frame_id.clone(),
                logical_frame_ordinal: frame.logical_frame_ordinal,
                topic: frame.topic.clone(),
                subscription_id: frame.subscription_id.clone(),
            },
        }
    }
}

struct FragmentAssembly {
    meta: WireMeta,
    fragment_count: u64,
    logical_bytes: u64,
    checksum: String,
    fragments: Vec<Option<Vec<u8>>>,
    received_count: u64,
    decoded_bytes: u64,
    first_seen_at: i64,
}

#[derive(Clone)]
struct SettledFrame {
    logical_frame_id: String,
    logical_frame_ordinal: u64,
}

pub struct SessionsIndexWireAssembler {
    assemblies: HashMap<String, FragmentAssembly>,
    /// 每 route 的单调 ordinal tombstone；不会被 bounded id LRU 淘汰后复活旧帧。
    settled_by_route: HashMap<String, SettledFrame>,
    staged_decoded_bytes: usize,
    timeout_ms: i64,
}

impl Default for SessionsIndexWireAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionsIndexWireAssembler {
    pub fn new() -> Self {
        Self::with_timeout(ASSEMBLY_TIMEOUT_MS)
    }

    pub fn with_timeout(timeout_ms: i64) -> Self {
        Self {
            assemblies: HashMap::new(),
            settled_by_route: HashMap::new(),
            staged_decoded_bytes: 0,
            timeout_ms,
        }
    }

    pub fn accept(
        &mut self,
        wire: &SessionsIndexWireFrame,
        now_ms: i64,
    ) -> Vec<SessionIndexAssemblyEvent> {
        let mut events = self.expire(now_ms);
        let meta = WireMeta::from_wire(wire);
        let key = route_key(&meta.topic, &meta.subscription_id);
        if meta.logical_frame_ordinal < 1 {
            events.push(fault(&meta, FAULT_METADATA_MISMATCH));
            return events;
        }

        // ordinal tombstone 检查必须早于 envelope/base64：迟到旧坏片不得释放新帧。
        if let Some(settled) = self.settled_by_route.get(&key) {
            if meta.logical_frame_ordinal < settled.logical_frame_ordinal {
                return events;
            }
            if meta.logical_frame_ordinal == settled.logical_frame_ordinal {
                if meta.logical_frame_id != settled.logical_frame_id {
                    events.push(fault(&meta, FAULT_ORDINAL_CONFLICT));
                }
                return events;
            }
        }

        if meta.delivery_kind == WireDeliveryKind::Recovery {
            // 权威 recovery 取代旧 assembly：同批的旧超时 fault 会误关 route。
            events.retain(|event| match event {
                SessionIndexAssemblyEvent::Fault(fault) => {
                    fault.topic != meta.topic
                        || fault.subscription_id != meta.subscription_id
                        || fault.logical_frame_ordinal >= meta.logical_frame_ordinal
                }
                SessionIndexAssemblyEvent::Complete { .. } => true,
            });
        }

        if let Some(active) = self.assemblies.get(&key) {
            if meta.logical_frame_ordinal < active.meta.logical_frame_ordinal {
                return events;
            }
            if meta.logical_frame_ordinal == active.meta.logical_frame_ordinal {
                if meta.logical_frame_id != active.meta.logical_frame_id {
                    let active = self.take_assembly(&key).expect("active assembly");
                    self.settle(&key, &active.meta);
                    events.push(fault(&meta, FAULT_ORDINAL_CONFLICT));
                    return events;
                }
            } else {
                let active = self.take_assembly(&key).expect("active assembly");
                self.settle(&key, &active.meta);
                // 只有权威 recovery 可无 fault supersede 残缺旧帧。
                if meta.delivery_kind != WireDeliveryKind::Recovery {
                    events.push(fault(&active.meta, FAULT_SUPERSEDED));
                }
            }
        }

        match wire {
            SessionsIndexWireFrame::Complete(frame) => {
                if envelope_bytes(wire) > ASSEMBLY_MAX_PHYSICAL_BYTES {
                    self.release_and_settle(&key, &meta);
                    events.push(fault(&meta, FAULT_ENVELOPE_TOO_LARGE));
                    return events;
                }
                if let Some(active) = self.take_assembly(&key) {
                    self.settle(&key, &active.meta);
                    events.push(fault(&active.meta, FAULT_METADATA_MISMATCH));
                    return events;
                }
                let logical_bytes = serde_json::to_vec(&frame.frame)
                    .map(|bytes| bytes.len() as u64)
                    .unwrap_or(u64::MAX);
                if logical_bytes > ASSEMBLY_MAX_BYTES {
                    self.settle(&key, &meta);
                    events.push(fault(&meta, FAULT_ASSEMBLY_TOO_LARGE));
                    return events;
                }
                if frame.frame.topic != meta.topic
                    || frame.frame.subscription_id != meta.subscription_id
                {
                    self.settle(&key, &meta);
                    events.push(fault(&meta, FAULT_METADATA_MISMATCH));
                    return events;
                }
                if let Err(_message) = frame.frame.validate() {
                    self.settle(&key, &meta);
                    events.push(fault(&meta, FAULT_INVALID_PAYLOAD));
                    return events;
                }
                self.settle(&key, &meta);
                events.push(SessionIndexAssemblyEvent::Complete {
                    frame: frame.frame.clone(),
                    delivery_kind: frame.delivery_kind,
                });
                events
            }
            SessionsIndexWireFrame::Fragment(fragment) => {
                if envelope_bytes(wire) > ASSEMBLY_MAX_PHYSICAL_BYTES {
                    self.release_and_settle(&key, &meta);
                    events.push(fault(&meta, FAULT_ENVELOPE_TOO_LARGE));
                    return events;
                }
                if fragment.fragment_count > ASSEMBLY_MAX_FRAGMENTS {
                    self.release_and_settle(&key, &meta);
                    events.push(fault(&meta, FAULT_FRAGMENT_COUNT_EXCEEDED));
                    return events;
                }
                if fragment.logical_bytes > ASSEMBLY_MAX_BYTES {
                    self.release_and_settle(&key, &meta);
                    events.push(fault(&meta, FAULT_ASSEMBLY_TOO_LARGE));
                    return events;
                }
                if fragment.fragment_count < 1
                    || fragment.fragment_index >= fragment.fragment_count
                    || fragment.logical_bytes < 1
                    || fragment.fragment_count > fragment.logical_bytes
                    || !is_crc32_hex(&fragment.checksum.value)
                {
                    self.release_and_settle(&key, &meta);
                    events.push(fault(&meta, FAULT_METADATA_MISMATCH));
                    return events;
                }
                let Ok(decoded) = base64::engine::general_purpose::STANDARD
                    .decode(fragment.data_base64.as_bytes())
                else {
                    self.release_and_settle(&key, &meta);
                    events.push(fault(&meta, FAULT_INVALID_BASE64));
                    return events;
                };

                if let Some(assembly) = self.assemblies.get(&key) {
                    if assembly.fragment_count != fragment.fragment_count
                        || assembly.meta.delivery_kind != meta.delivery_kind
                        || assembly.logical_bytes != fragment.logical_bytes
                        || assembly.checksum != fragment.checksum.value
                    {
                        let active = self.take_assembly(&key).expect("active assembly");
                        self.settle(&key, &active.meta);
                        events.push(fault(&meta, FAULT_METADATA_MISMATCH));
                        return events;
                    }
                } else {
                    if self.assemblies.len() >= ASSEMBLY_MAX_CONCURRENT {
                        self.settle(&key, &meta);
                        events.push(fault(&meta, FAULT_CONCURRENT_LIMIT));
                        return events;
                    }
                    if self.staged_decoded_bytes + decoded.len() > ASSEMBLY_MAX_STAGED_BYTES {
                        self.settle(&key, &meta);
                        events.push(fault(&meta, FAULT_BUDGET_EXCEEDED));
                        return events;
                    }
                    self.assemblies.insert(
                        key.clone(),
                        FragmentAssembly {
                            meta: meta.clone(),
                            fragment_count: fragment.fragment_count,
                            logical_bytes: fragment.logical_bytes,
                            checksum: fragment.checksum.value.clone(),
                            fragments: vec![None; fragment.fragment_count as usize],
                            received_count: 0,
                            decoded_bytes: 0,
                            first_seen_at: now_ms,
                        },
                    );
                }

                let assembly = self.assemblies.get_mut(&key).expect("assembly");
                let index = fragment.fragment_index as usize;
                if let Some(previous) = &assembly.fragments[index] {
                    if previous != &decoded {
                        let active = self.take_assembly(&key).expect("active assembly");
                        self.settle(&key, &active.meta);
                        events.push(fault(&meta, FAULT_FRAGMENT_CONFLICT));
                    }
                    return events;
                }
                if self.staged_decoded_bytes + decoded.len() > ASSEMBLY_MAX_STAGED_BYTES {
                    let active = self.take_assembly(&key).expect("active assembly");
                    self.settle(&key, &active.meta);
                    events.push(fault(&meta, FAULT_BUDGET_EXCEEDED));
                    return events;
                }
                if assembly.decoded_bytes + decoded.len() as u64 > assembly.logical_bytes {
                    let active = self.take_assembly(&key).expect("active assembly");
                    self.settle(&key, &active.meta);
                    events.push(fault(&meta, FAULT_LENGTH_MISMATCH));
                    return events;
                }
                assembly.fragments[index] = Some(decoded.clone());
                assembly.received_count += 1;
                assembly.decoded_bytes += decoded.len() as u64;
                self.staged_decoded_bytes += decoded.len();

                if assembly.received_count != assembly.fragment_count {
                    return events;
                }

                let assembly = self.take_assembly(&key).expect("active assembly");
                self.finish_assembly(key, assembly, &mut events);
                events
            }
        }
    }

    pub fn expire(&mut self, now_ms: i64) -> Vec<SessionIndexAssemblyEvent> {
        let expired: Vec<String> = self
            .assemblies
            .iter()
            .filter(|(_, assembly)| now_ms - assembly.first_seen_at >= self.timeout_ms)
            .map(|(key, _)| key.clone())
            .collect();
        let mut events = Vec::new();
        for key in expired {
            let Some(assembly) = self.take_assembly(&key) else {
                continue;
            };
            self.settle(&key, &assembly.meta);
            events.push(fault(&assembly.meta, FAULT_TIMED_OUT));
        }
        events
    }

    /// 丢弃 route（含 tombstone）：用于订阅释放/换代。
    pub fn discard(&mut self, topic: &str, subscription_id: &str) {
        let key = route_key(topic, subscription_id);
        self.take_assembly(&key);
        self.settled_by_route.remove(&key);
    }

    /// fault 后释放 active bytes，但保留 ordinal tombstone 防旧 replay 复活。
    pub fn abort(&mut self, topic: &str, subscription_id: &str) {
        let key = route_key(topic, subscription_id);
        if let Some(assembly) = self.take_assembly(&key) {
            self.settle(&key, &assembly.meta);
        }
    }

    pub fn clear(&mut self) {
        self.assemblies.clear();
        self.settled_by_route.clear();
        self.staged_decoded_bytes = 0;
    }

    pub fn stats(&self) -> (usize, usize) {
        (self.assemblies.len(), self.staged_decoded_bytes)
    }

    fn finish_assembly(
        &mut self,
        key: String,
        assembly: FragmentAssembly,
        events: &mut Vec<SessionIndexAssemblyEvent>,
    ) {
        if assembly.decoded_bytes != assembly.logical_bytes {
            self.settle(&key, &assembly.meta);
            events.push(fault(&assembly.meta, FAULT_LENGTH_MISMATCH));
            return;
        }
        let mut logical = Vec::with_capacity(assembly.decoded_bytes as usize);
        for fragment in &assembly.fragments {
            let Some(fragment) = fragment else {
                self.settle(&key, &assembly.meta);
                events.push(fault(&assembly.meta, FAULT_LENGTH_MISMATCH));
                return;
            };
            logical.extend_from_slice(fragment);
        }
        let checksum = format!("{:08x}", crc32fast::hash(&logical));
        if checksum != assembly.checksum {
            self.settle(&key, &assembly.meta);
            events.push(fault(&assembly.meta, FAULT_CHECKSUM_MISMATCH));
            return;
        }
        let Ok(json) = String::from_utf8(logical) else {
            self.settle(&key, &assembly.meta);
            events.push(fault(&assembly.meta, FAULT_INVALID_UTF8));
            return;
        };
        let Ok(value) = serde_json::from_str::<Value>(&json) else {
            self.settle(&key, &assembly.meta);
            events.push(fault(&assembly.meta, FAULT_INVALID_JSON));
            return;
        };
        let envelope_matches = value
            .get("topic")
            .and_then(Value::as_str)
            .is_some_and(|topic| topic == assembly.meta.topic)
            && value
                .get("subscriptionId")
                .and_then(Value::as_str)
                .is_some_and(|subscription_id| subscription_id == assembly.meta.subscription_id);
        if !envelope_matches {
            self.settle(&key, &assembly.meta);
            events.push(fault(&assembly.meta, FAULT_METADATA_MISMATCH));
            return;
        }
        match serde_json::from_value::<SessionsIndexTopicFrame>(value) {
            Ok(frame) if frame.validate().is_ok() => {
                self.settle(&key, &assembly.meta);
                events.push(SessionIndexAssemblyEvent::Complete {
                    frame,
                    delivery_kind: assembly.meta.delivery_kind,
                });
            }
            _ => {
                self.settle(&key, &assembly.meta);
                events.push(fault(&assembly.meta, FAULT_INVALID_PAYLOAD));
            }
        }
    }

    fn take_assembly(&mut self, key: &str) -> Option<FragmentAssembly> {
        let assembly = self.assemblies.remove(key)?;
        self.staged_decoded_bytes = self
            .staged_decoded_bytes
            .saturating_sub(assembly.decoded_bytes as usize);
        Some(assembly)
    }

    fn settle(&mut self, key: &str, meta: &WireMeta) {
        if let Some(previous) = self.settled_by_route.get(key) {
            if previous.logical_frame_ordinal > meta.logical_frame_ordinal {
                return;
            }
        }
        self.settled_by_route.insert(
            key.to_string(),
            SettledFrame {
                logical_frame_id: meta.logical_frame_id.clone(),
                logical_frame_ordinal: meta.logical_frame_ordinal,
            },
        );
    }

    fn release_and_settle(&mut self, key: &str, meta: &WireMeta) {
        if let Some(active) = self.assemblies.get(key) {
            if active.meta.logical_frame_ordinal == meta.logical_frame_ordinal
                && active.meta.logical_frame_id == meta.logical_frame_id
            {
                self.take_assembly(key);
            }
        }
        self.settle(key, meta);
    }
}

fn route_key(topic: &str, subscription_id: &str) -> String {
    format!("{topic}\u{0}{subscription_id}")
}

fn envelope_bytes(wire: &SessionsIndexWireFrame) -> usize {
    serde_json::to_vec(wire).map(|bytes| bytes.len()).unwrap_or(usize::MAX)
}

fn is_crc32_hex(value: &str) -> bool {
    value.len() == 8
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn fault(meta: &WireMeta, reason_code: &'static str) -> SessionIndexAssemblyEvent {
    SessionIndexAssemblyEvent::Fault(SessionIndexAssemblyFault {
        delivery_kind: meta.delivery_kind,
        reason_code,
        logical_frame_id: meta.logical_frame_id.clone(),
        logical_frame_ordinal: meta.logical_frame_ordinal,
        topic: meta.topic.clone(),
        subscription_id: meta.subscription_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Number;

    use crate::v4_sessions_index::{
        SessionsIndexCompleteWireFrame, SessionsIndexDeltasPayload, SessionsIndexFramePayload,
        SessionsIndexSnapshot, SessionsIndexSnapshotPayload, SessionsIndexTopicFrame,
    };
    use crate::v4_transport::{
        ChecksumAlgorithm, FragmentWireFrame, FramePayloadKindDeltas, FramePayloadKindSnapshot,
        TopicWireChecksum, WireFrameKindComplete, WireFrameKindFragment,
    };

    const TOPIC: &str = "sessions-index/ws-1";
    const SUB: &str = "sub-1";

    fn deltas_frame(to_seq: i64) -> SessionsIndexTopicFrame {
        SessionsIndexTopicFrame {
            topic: TOPIC.to_string(),
            subscription_id: SUB.to_string(),
            from_seq: Number::from(0),
            to_seq: Number::from(to_seq),
            sent_at: Number::from(1),
            payload: SessionsIndexFramePayload::Deltas(SessionsIndexDeltasPayload {
                kind: FramePayloadKindDeltas::Deltas,
                deltas: Vec::new(),
            }),
        }
    }

    fn snapshot_frame(protocol_version: u64) -> SessionsIndexTopicFrame {
        SessionsIndexTopicFrame {
            topic: TOPIC.to_string(),
            subscription_id: SUB.to_string(),
            from_seq: Number::from(0),
            to_seq: Number::from(0),
            sent_at: Number::from(1),
            payload: SessionsIndexFramePayload::Snapshot(Box::new(SessionsIndexSnapshotPayload {
                kind: FramePayloadKindSnapshot::Snapshot,
                snapshot: SessionsIndexSnapshot {
                    protocol_version,
                    workspace_id: "ws-1".to_string(),
                    log_epoch: "epoch-1".to_string(),
                    sessions: Vec::new(),
                },
            })),
        }
    }

    fn complete(
        id: &str,
        ordinal: u64,
        delivery_kind: WireDeliveryKind,
        frame: SessionsIndexTopicFrame,
    ) -> SessionsIndexWireFrame {
        SessionsIndexWireFrame::Complete(Box::new(SessionsIndexCompleteWireFrame {
            wire_version: 1,
            kind: WireFrameKindComplete::Complete,
            delivery_kind,
            logical_frame_id: id.to_string(),
            logical_frame_ordinal: ordinal,
            topic: TOPIC.to_string(),
            subscription_id: SUB.to_string(),
            frame,
        }))
    }

    fn fragments(
        id: &str,
        ordinal: u64,
        delivery_kind: WireDeliveryKind,
        frame: &SessionsIndexTopicFrame,
        chunk_count: usize,
    ) -> Vec<SessionsIndexWireFrame> {
        let logical = serde_json::to_vec(frame).expect("frame json");
        let checksum = format!("{:08x}", crc32fast::hash(&logical));
        let chunk_size = logical.len().div_ceil(chunk_count).max(1);
        let parts: Vec<&[u8]> = logical.chunks(chunk_size).collect();
        parts
            .iter()
            .enumerate()
            .map(|(index, part)| {
                SessionsIndexWireFrame::Fragment(FragmentWireFrame {
                    wire_version: 1,
                    kind: WireFrameKindFragment::Fragment,
                    delivery_kind,
                    logical_frame_id: id.to_string(),
                    logical_frame_ordinal: ordinal,
                    topic: TOPIC.to_string(),
                    subscription_id: SUB.to_string(),
                    fragment_index: index as u64,
                    fragment_count: parts.len() as u64,
                    logical_bytes: logical.len() as u64,
                    checksum: TopicWireChecksum {
                        algorithm: ChecksumAlgorithm::Crc32,
                        value: checksum.clone(),
                    },
                    data_base64: base64::engine::general_purpose::STANDARD.encode(part),
                })
            })
            .collect()
    }

    fn reasons(events: &[SessionIndexAssemblyEvent]) -> Vec<&'static str> {
        events
            .iter()
            .filter_map(|event| match event {
                SessionIndexAssemblyEvent::Fault(fault) => Some(fault.reason_code),
                SessionIndexAssemblyEvent::Complete { .. } => None,
            })
            .collect()
    }

    #[test]
    fn complete_frame_accepted_then_tombstoned() {
        let mut assembler = SessionsIndexWireAssembler::new();
        let wire = complete("frame-1", 1, WireDeliveryKind::Initial, deltas_frame(1));
        let events = assembler.accept(&wire, 0);
        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionIndexAssemblyEvent::Complete {
                frame,
                delivery_kind,
            } => {
                assert_eq!(frame.to_seq, Number::from(1));
                assert_eq!(*delivery_kind, WireDeliveryKind::Initial);
            }
            other => panic!("expected complete, got {other:?}"),
        }
        // 同 ordinal 同 id 重放：吞掉。
        assert!(assembler.accept(&wire, 0).is_empty());
        // 旧 ordinal（>=1）迟到：tombstone 吞掉。
        assembler.accept(
            &complete("frame-2", 2, WireDeliveryKind::Online, deltas_frame(2)),
            0,
        );
        let older = complete("frame-1b", 1, WireDeliveryKind::Online, deltas_frame(1));
        assert!(assembler.accept(&older, 0).is_empty());
        // ordinal < 1 是协议违规：metadata mismatch。
        let invalid = complete("frame-0", 0, WireDeliveryKind::Online, deltas_frame(1));
        assert_eq!(
            reasons(&assembler.accept(&invalid, 0)),
            vec![FAULT_METADATA_MISMATCH]
        );
    }

    #[test]
    fn ordinal_conflict_and_invalid_payload() {
        let mut assembler = SessionsIndexWireAssembler::new();
        assembler.accept(
            &complete("frame-1", 1, WireDeliveryKind::Online, deltas_frame(1)),
            0,
        );
        let conflict = assembler.accept(
            &complete("frame-1b", 1, WireDeliveryKind::Online, deltas_frame(2)),
            0,
        );
        assert_eq!(reasons(&conflict), vec![FAULT_ORDINAL_CONFLICT]);

        // payload snapshot protocolVersion != 1 → invalid payload。
        let mut wire = complete("frame-2", 2, WireDeliveryKind::Online, snapshot_frame(2));
        if let SessionsIndexWireFrame::Complete(frame) = &mut wire {
            frame.frame.to_seq = Number::from(2);
        }
        let events = assembler.accept(&wire, 0);
        assert_eq!(reasons(&events), vec![FAULT_INVALID_PAYLOAD]);
    }

    #[test]
    fn fragments_assemble_into_complete_frame() {
        let mut assembler = SessionsIndexWireAssembler::new();
        let frame = deltas_frame(7);
        let wires = fragments("frame-1", 1, WireDeliveryKind::Online, &frame, 3);
        assert!(wires.len() > 1);
        let mut completed = 0;
        for wire in &wires {
            for event in assembler.accept(wire, 0) {
                if let SessionIndexAssemblyEvent::Complete { frame, .. } = event {
                    assert_eq!(frame.to_seq, Number::from(7));
                    completed += 1;
                }
            }
        }
        assert_eq!(completed, 1);
        let (active, staged) = assembler.stats();
        assert_eq!(active, 0);
        assert_eq!(staged, 0);
    }

    #[test]
    fn checksum_mismatch_faults_and_clears() {
        let mut assembler = SessionsIndexWireAssembler::new();
        let frame = deltas_frame(3);
        let mut wires = fragments("frame-1", 1, WireDeliveryKind::Online, &frame, 2);
        // 篡改分片内容（保持元数据一致）：最终 crc32 不匹配。
        if let SessionsIndexWireFrame::Fragment(fragment) = &mut wires[1] {
            let mut bytes = base64::engine::general_purpose::STANDARD
                .decode(fragment.data_base64.as_bytes())
                .expect("decode");
            bytes[0] ^= 0xff;
            fragment.data_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        }
        let mut events = Vec::new();
        for wire in &wires {
            events.extend(assembler.accept(wire, 0));
        }
        assert_eq!(reasons(&events), vec![FAULT_CHECKSUM_MISMATCH]);
        assert_eq!(assembler.stats().0, 0);
    }

    #[test]
    fn recovery_supersedes_without_superseded_fault() {
        let mut assembler = SessionsIndexWireAssembler::new();
        // online 残缺帧（只到第一片）。
        let online = fragments("frame-1", 1, WireDeliveryKind::Online, &deltas_frame(1), 2);
        assert!(assembler.accept(&online[0], 0).is_empty());
        // 更高 ordinal 的 recovery：无 Superseded fault。
        let recovery = complete("frame-2", 2, WireDeliveryKind::Recovery, deltas_frame(2));
        let events = assembler.accept(&recovery, 1);
        assert!(reasons(&events).is_empty(), "{:?}", reasons(&events));
        assert!(events
            .iter()
            .any(|event| matches!(event, SessionIndexAssemblyEvent::Complete { .. })));

        // 非 recovery 的更高 ordinal 取代残片：记 Superseded。
        let mut assembler = SessionsIndexWireAssembler::new();
        let online = fragments("frame-1", 1, WireDeliveryKind::Online, &deltas_frame(1), 2);
        assert!(assembler.accept(&online[0], 0).is_empty());
        let newer = complete("frame-2", 2, WireDeliveryKind::Online, deltas_frame(2));
        let events = assembler.accept(&newer, 1);
        assert_eq!(reasons(&events), vec![FAULT_SUPERSEDED]);
    }

    #[test]
    fn timeout_and_fragment_conflict() {
        let mut assembler = SessionsIndexWireAssembler::with_timeout(10);
        let frame = deltas_frame(1);
        let wires = fragments("frame-1", 1, WireDeliveryKind::Online, &frame, 2);
        assert!(assembler.accept(&wires[0], 0).is_empty());
        // 超时：下一次 accept 先 expire。
        let events = assembler.accept(&wires[0], 20);
        assert!(
            reasons(&events).contains(&FAULT_TIMED_OUT),
            "{:?}",
            reasons(&events)
        );

        // 同片重放相同 → 忽略；内容不同 → FragmentConflict。
        let mut assembler = SessionsIndexWireAssembler::new();
        let wires = fragments("frame-1", 1, WireDeliveryKind::Online, &frame, 2);
        assert!(assembler.accept(&wires[0], 0).is_empty());
        assert!(assembler.accept(&wires[0], 0).is_empty());
        let mut tampered = wires[0].clone();
        if let SessionsIndexWireFrame::Fragment(fragment) = &mut tampered {
            fragment.data_base64 =
                base64::engine::general_purpose::STANDARD.encode(b"{\"topic\":");
        }
        let events = assembler.accept(&tampered, 0);
        assert_eq!(reasons(&events), vec![FAULT_FRAGMENT_CONFLICT]);
        assert_eq!(assembler.stats().0, 0);
    }
}

