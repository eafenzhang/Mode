//! 帧路由：按频道归属把页面来帧分成「壳内处理」与「交给遗留 Node Host」。
//!
//! 归属是**启动期**决定的（`FrameRouter::new` 传入本地频道集合），运行时不热切，
//! 保证同一频道同一时刻只有一个所有者。路由不修改字节：判定为 `Legacy` 的帧由调用方
//! 原样转发（含 scoped 前缀）。

use std::collections::HashSet;

use rpc_wire::scope::{
    decode_control_frame, decode_frame, AttachmentControl, CONTROL_FRAME_MARKER,
};
use rpc_wire::{decode_rpc_frame, RpcFrame, RpcMessage};

#[derive(Debug, Clone, PartialEq)]
pub enum FrameRoute {
    /// 附件控制帧（scope-ready / scope-detach），由调用方处理 attachment 生命周期。
    Control(AttachmentControl),
    /// 本地（Rust）服务面拥有的请求。
    ///
    /// `payload` 是**剥掉 scope 前缀后的原始字节**：直接交给 `ChannelServer`，
    /// 避免「解析后再编码」带来的字节差异（对象分支的 JSON 文本要原样保留）。
    Local {
        scope_id: Option<u32>,
        request: RpcMessage,
        payload: Vec<u8>,
    },
    /// 不属于本地频道：原样转发给遗留 Host（保留 scoped 前缀）。
    Legacy,
    /// 畸形帧或不是请求：调用方丢弃并计数（对应 TS 的 `frame_rejected`）。
    Rejected,
}

pub struct FrameRouter {
    owned: HashSet<String>,
}

impl FrameRouter {
    pub fn new(owned: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            owned: owned.into_iter().map(Into::into).collect(),
        }
    }

    pub fn owns(&self, channel: &str) -> bool {
        self.owned.contains(channel)
    }

    pub fn owned_channels(&self) -> impl Iterator<Item = &str> {
        self.owned.iter().map(String::as_str)
    }

    pub fn route(&self, frame: &[u8]) -> FrameRoute {
        let (scope_id, payload) = match decode_frame(frame) {
            Ok(rpc_wire::DecodedFrame::Base(payload)) => (None, payload),
            Ok(rpc_wire::DecodedFrame::Scoped { scope_id, payload }) => (Some(scope_id), payload),
            // 与 TS demux 一致：畸形 scoped 头直接丢弃。
            Err(_) => return FrameRoute::Rejected,
        };

        // 控制帧先于 RPC 校验：它不属于任何 scope，也不承载 RPC。
        if payload.first() == Some(&CONTROL_FRAME_MARKER) {
            return match decode_control_frame(payload) {
                Some(control) => FrameRoute::Control(control),
                None => FrameRoute::Rejected,
            };
        }

        match decode_rpc_frame(payload) {
            Ok(RpcFrame::Request(request)) => match request.header.channel() {
                Some(channel) if self.owned.contains(channel) => FrameRoute::Local {
                    scope_id,
                    request,
                    payload: payload.to_vec(),
                },
                // 本地不认识的请求（含 cancel/dispose）交给遗留 Host：那里知道 id 对应什么。
                _ => FrameRoute::Legacy,
            },
            // 回帧、未知请求类型、无法解析：与 TS Host 入口一致，判为不可处理。
            _ => FrameRoute::Rejected,
        }
    }
}
