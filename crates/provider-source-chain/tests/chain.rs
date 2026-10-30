//! 链路编排：文件层 → 账号 Overlay → 交付快照，以及失败语义。

mod support;

use std::sync::atomic::Ordering;

use provider_source_chain::{AgentProviderSource, ChainError, ProviderChain};
use support::{Fixture, FakeOptions, REVISION};

use agent_process::ProviderSource;

#[tokio::test]
async fn resolves_account_overlay_and_is_stable_across_reads() {
    let fixture = Fixture::new("stable");
    let mut chain = ProviderChain::new(
        fixture.bundled.clone(),
        None,
        fixture.personal.clone(),
        FakeOptions::ready(),
    );

    let snapshot = chain.resolve(&["start".to_owned()]).await.unwrap();
    assert!(
        snapshot.revision.starts_with("account:["),
        "unexpected revision {}",
        snapshot.revision
    );
    assert!(
        snapshot
            .based_on_mode_builtin_revision
            .starts_with(&format!("mode-builtin:{REVISION}:")),
        "unexpected basedOn {}",
        snapshot.based_on_mode_builtin_revision
    );

    let agent = provider_source_chain::to_agent_snapshot(&snapshot);
    let start = &agent.providers["zai-start"];
    // Account Overlay 只发布运行时事实：accountType/mode 属于 Built-in 配置，不重复下发。
    assert_eq!(start["access"]["type"], "zhipu-account");
    assert_eq!(start["access"]["entitled"], true);
    assert!(start["access"].get("accountType").is_none());
    assert_eq!(start["builtinModelIds"], serde_json::json!([]));
    assert!(agent.providers["zai-individual"]
        .get("builtinModelIds")
        .is_none());
    assert_eq!(agent.states["zai-start"]["availability"], "available");
    assert_eq!(agent.states["zai-start"]["entitled"], true);
    assert_eq!(agent.states["zai-start"]["current"], true);
    // Account State 不携带 models；非 Start Plan 的白名单不进入 Overlay。
    assert_eq!(
        agent.states["zai-individual"]["availability"],
        "available"
    );

    let again = chain.resolve(&["explicit".to_owned()]).await.unwrap();
    assert_eq!(again.revision, snapshot.revision);
}

#[tokio::test]
async fn first_failure_publishes_fail_closed() {
    let fixture = Fixture::new("fail-closed");
    let source = AgentProviderSource::new(
        fixture.bundled.clone(),
        None,
        fixture.personal.clone(),
        FakeOptions::failing(),
    );
    let snapshot = source.read().await.unwrap();

    assert!(snapshot.providers.contains_key("zai-start"));
    assert_eq!(snapshot.providers["zai-start"]["access"]["entitled"], false);
    assert!(snapshot.states.is_empty());
}

#[tokio::test]
async fn later_failure_keeps_last_known_good_and_reports_error() {
    let fixture = Fixture::new("later-failure");
    let options = FakeOptions::ready();
    let fail_handle = options.fail.clone();
    let mut chain = ProviderChain::new(
        fixture.bundled.clone(),
        None,
        fixture.personal.clone(),
        options,
    );

    let first = chain.resolve(&["start".to_owned()]).await.unwrap();
    fail_handle.store(true, Ordering::SeqCst);
    let error = chain.resolve(&["explicit".to_owned()]).await.unwrap_err();
    assert!(matches!(error, ChainError::Overlay(_)), "unexpected {error:?}");
    assert_eq!(
        chain
            .last_snapshot()
            .map(|snapshot| snapshot.revision.clone()),
        Some(first.revision.clone())
    );

    fail_handle.store(false, Ordering::SeqCst);
    let recovered = chain.resolve(&["explicit".to_owned()]).await.unwrap();
    assert_eq!(recovered.revision, first.revision);
}

#[tokio::test]
async fn missing_bundled_release_is_a_config_error() {
    let fixture = Fixture::new("missing-bundled");
    let mut chain = ProviderChain::new(
        fixture.dir.join("nope.json").to_string_lossy().into_owned(),
        None,
        fixture.personal.clone(),
        FakeOptions::ready(),
    );
    let error = chain.resolve(&["start".to_owned()]).await.unwrap_err();
    assert!(matches!(error, ChainError::Config(_)), "unexpected {error:?}");
    assert!(chain.last_snapshot().is_none());
}
