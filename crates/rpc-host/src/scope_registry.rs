//! 附件 scope 的挂接表（为「按 scope 复用服务面」铺路）。
//!
//! 真源语义见 `packages/rpc/src/frameMultiplexer.ts`：`scope-ready` 挂接、`scope-detach`
//! 回收；之后该 scope 的帧才有资格被投递。壳内服务面目前只服务 base scope，这里先把
//! 「哪些 scope 处于挂接态」与「回帧如何加前缀」做成可测的纯状态。

use std::collections::HashSet;

use rpc_wire::scope::encode_scoped_frame;

#[derive(Default)]
pub struct LocalScopeRegistry {
    attached: HashSet<u32>,
}

impl LocalScopeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回 `false` 表示重复挂接（与 TS 的行为一致：重复绑定是配置错误）。
    pub fn attach(&mut self, scope_id: u32) -> bool {
        self.attached.insert(scope_id)
    }

    pub fn detach(&mut self, scope_id: u32) -> bool {
        self.attached.remove(&scope_id)
    }

    pub fn is_attached(&self, scope_id: u32) -> bool {
        self.attached.contains(&scope_id)
    }

    pub fn len(&self) -> usize {
        self.attached.len()
    }

    pub fn is_empty(&self) -> bool {
        self.attached.is_empty()
    }

    /// 给该 scope 的回帧加 `0xFF + scopeId` 前缀；未挂接的 scope 返回 `None`
    /// （调用方必须丢弃，不能按 base 帧发出去）。
    pub fn wrap_response(&self, scope_id: u32, payload: &[u8]) -> Option<Vec<u8>> {
        if !self.is_attached(scope_id) {
            return None;
        }
        encode_scoped_frame(scope_id, payload).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_attached_scopes_can_produce_prefixed_frames() {
        let mut registry = LocalScopeRegistry::new();
        assert!(registry.wrap_response(7, b"x").is_none(), "未挂接必须拒绝");

        assert!(registry.attach(7));
        assert!(!registry.attach(7), "重复挂接返回 false");
        let wrapped = registry.wrap_response(7, b"x").expect("已挂接");
        assert_eq!(wrapped[0], 0xff);
        assert_eq!(&wrapped[1..5], &7u32.to_be_bytes());

        assert!(registry.detach(7));
        assert!(registry.wrap_response(7, b"x").is_none());
        assert!(registry.is_empty());
    }
}
