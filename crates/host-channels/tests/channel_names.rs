//! 频道名与 TS `ServiceChannels` 的漂移守卫。
//!
//! 壳内适配器把频道名写成常量；TS 侧改名字时这里必须一起红——否则壳会「接管一个不存在的
//! 频道」，而渲染层调用会落到 Node 侧，表现为「开关没生效」。

const CHANNELS_TS: &str = include_str!("../../../packages/shared/src/channels.ts");

#[test]
fn every_local_channel_name_exists_in_service_channels() {
    let names = [
        ("memory", "Memory"),
        ("prompt-attachment-transfer", "PromptAttachmentTransfer"),
        ("client-config", "ClientConfig"),
        ("client-scenes", "ClientScenes"),
        ("media-preview", "MediaPreview"),
        ("conversation-mirror", "ConversationMirror"),
    ];
    for (value, key) in names {
        let expected = format!("{key}: \"{value}\"");
        assert!(
            CHANNELS_TS.contains(&expected),
            "TS ServiceChannels 里找不到 `{expected}`；改名后必须同步 crates/host-channels 的常量"
        );
    }
}

#[test]
fn local_channel_names_are_unique() {
    let names = [
        host_channels::MEMORY_CHANNEL,
        host_channels::ATTACHMENT_TRANSFER_CHANNEL,
        host_channels::CLIENT_CONFIG_CHANNEL,
        host_channels::CLIENT_SCENES_CHANNEL,
        host_channels::MEDIA_PREVIEW_CHANNEL,
        host_channels::CONVERSATION_MIRROR_CHANNEL,
    ];
    let mut sorted = names.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "本地频道名不能重复");
}
