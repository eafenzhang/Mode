//! M3 Task 3 · Step 4 的 **opt-in 真 webview harness**（**测试资产**，不是产品入口）。
//!
//! 与 Task 1 的 `HostRoundtripHarness` 是**同一套产品部件**（`mode_tauri::host::start` +
//! `mode_tauri::build_host_window`），差别只有两处，而这两处正是本任务要验的东西：
//!
//! 1. 加载的是 **`index.html`**——真 renderer 的构建产物（`frontendDist` 装配后的产品页），
//!    不是集成测试页；
//! 2. **追加一段探针**注入脚本（`test/harness/product-ui/probe.ts`）。页面不能改
//!    （renderer 源码零改动是硬约束），所以观测只能从注入做。
//!
//! 它还有第二种用途：**只撤掉 `window.mode` 的负向对照**（`MODE_PRODUCT_UI_WITHHOLD_WINDOW_MODE=1`）。
//! 那一条把「renderer 在入口模块求值期消费 `window.mode`，缺席就抛」从**读实现体的推论**变成
//! **观察到的行为**——clean-room 复评要求的就是这个（见 `host_bridge_script` 的注释与 spec §19）。
//!
//! ```text
//! node scripts/rust-cargo.ts build -p mode-tauri --features product-ui-harness --bin ProductUiHarness
//! MODE_TAURI_PRODUCT_UI=1 node --import tsx --test apps/desktop-tauri/test/productUiMount.test.mts
//! ```
//! 通常不用手敲：`productUiMount.test.mts` 会自己装配前端、构建本 bin、起 sink 并断言结果。

use std::time::Duration;

use tauri::Manager;

/// 探针脚本（测试资产；**注入**而不是改页面，见文件头）。
///
/// 探针要知道本次跑的是**哪一档**（产品档 / 撤掉 `window.mode` 的负向档），因为两档的收工条件不同：
/// 产品档等挂载成功，负向档在**第一次页面错误**时就可以收工并如实报告。占位符由本文件替换。
const PROBE_SCRIPT: &str = include_str!("../../../test/harness/product-ui/probe.ts");

/// 是否撤掉 `window.mode` 的注入（负向对照）。
fn withhold_window_mode() -> bool {
    matches!(
        std::env::var("MODE_PRODUCT_UI_WITHHOLD_WINDOW_MODE").as_deref(),
        Ok("1")
    )
}

/// **敏感性对照**：强制给窗口加系统标题栏（默认关）。
///
/// 为什么要有这一档：`windowControls.test.mts` 拿「无边框」当用户可见的判据，而一条**看不出
/// 两种形态差别**的判据等于没有判据——本轮就漏过一次判据写反（产品窗口反而有系统栏）。
/// 这一档把窗口做成**有**系统栏，用例必须能在这一档上看出**相反**的结果。
fn force_decorations() -> bool {
    matches!(
        std::env::var("MODE_PRODUCT_UI_FORCE_DECORATIONS").as_deref(),
        Ok("1")
    )
}

/// 是否跑窗口控制序列（见 `PROBE_SCRIPT` 的 `WINDOW_CONTROLS`）。
fn window_controls() -> bool {
    matches!(std::env::var("MODE_PRODUCT_UI_WINDOW_CONTROLS").as_deref(), Ok("1"))
}

fn watchdog_ms() -> u64 {
    std::env::var("MODE_PRODUCT_UI_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60_000)
}

fn main() {
    // 与产品壳**同一批部件**，包括单实例插件（M3 Task 4）：`singleInstance.test.mts` 要它
    // 来充当「已运行的实例」——探针在页面里，所以观测点只能挂在 harness 上；而互斥体/隐藏窗口
    // 是按 app identifier 建的，产品壳与 harness 共用同一个 identifier，所以第二实例（同一个
    // 可执行文件 + 深链在 argv 里）能转发到本进程。注册顺序同样是「最先」（见 single_instance.rs）。
    let builder = mode_tauri::single_instance::install(tauri::Builder::default());
    let builder = platform::commands::init_with_extra(builder, mode_tauri::extra_handler());

    let builder = builder.setup(|app| {
        let handle = app.handle().clone();

        // 深链路由状态（**只托管、不注册 scheme**）：`singleInstance.test.mts` 要本进程充当
        // 「已在运行的实例」并把转发进来的链接投到页面上，而投递路径就是这份路由。
        // 刻意不用 `deeplink::install`——它会把 `mode://` 的默认 handler 指向本可执行文件，
        // 那是真机上的持久副作用（测试不该改用户的注册表）。见 `deeplink::manage_state`。
        deeplink::manage_state(&handle);

        // 产品接线：监督器起第一代；注入桥在文档启动时采用当前代（不再注入 id 字面量）。
        let supervisor = mode_tauri::host::start(&handle);

        // 与产品壳**同一段注入**（`bridge_script` 内含 `window.mode`），入口页也取产品默认值
        // （`pick_shell_entry` 在未覆写时返回 `index.html`）。
        let withhold = withhold_window_mode();
        let injection = if withhold {
            sidecar_host::bridge::host_bridge_script()
        } else {
            sidecar_host::bridge::bridge_script()
        };
        let probe = PROBE_SCRIPT
            .replace(
                "__MODE_PRODUCT_UI_EXPECT__",
                if withhold { "withheld" } else { "mounted" },
            )
            // 窗口控制序列是**可选档**（`windowControls.test.mts` 打开）：它最后会**真关窗**，
            // 所以默认档绝不能跑——那会让 productUiMount 那条用例的窗口半路消失。
            .replace(
                "__MODE_PRODUCT_UI_WINDOW_CONTROLS__",
                if window_controls() { "1" } else { "0" },
            );
        println!(
            "HARNESS injection={} startupId={} pid={}",
            if withhold { "bridge_only" } else { "window_mode+bridge" },
            supervisor.current().startup_id(),
            supervisor.current().pid()
        );
        app.manage(supervisor);
        // **产品页 → 无边框**：与产品壳同一个判据（`decorations_for_shell_entry(None)`），
        // 不在这里另写一遍——调用点自己写判据正是这次判错的地方。对照档见 `force_decorations`。
        let expected_decorations = force_decorations() || sidecar_host::decorations_for_shell_entry(None);
        let window = mode_tauri::build_host_window(
            &handle,
            mode_tauri::MAIN_WINDOW_LABEL,
            tauri::WebviewUrl::App(sidecar_host::pick_shell_entry(None)),
            "M3 Task 3 · product UI",
            &injection,
            Some(&probe),
            expected_decorations,
        )?;
        // 窗口形态的**运行时观察**：读窗口自己的 `is_decorated()`，不是读刚传进去的参数。
        // 「参数传对了」与「窗口真的那样」是两件事——判据写反那次正是前者看着对、后者是错的。
        match window.is_decorated() {
            Ok(actual) => println!(
                "HARNESS window decorations_expected={expected_decorations} decorations_actual={actual}"
            ),
            Err(error) => println!(
                "HARNESS window decorations_expected={expected_decorations} decorations_actual=unknown error={error}"
            ),
        }

        let exit_handle = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(watchdog_ms()));
            println!("HARNESS watchdog_exit");
            exit_handle.exit(0);
        });
        Ok(())
    });

    let app = builder
        .build(tauri::generate_context!())
        .expect("harness 壳构建失败");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(supervisor) =
                app_handle.try_state::<std::sync::Arc<mode_tauri::host::HostSupervisor>>()
            {
                let pid = supervisor.current().pid();
                supervisor.shutdown();
                println!("HARNESS host_shutdown pid={pid}");
            }
        }
    });
}
