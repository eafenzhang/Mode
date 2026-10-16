//! M3 Task 0「端口传输」harness 的壳侧入口（**测试资产**，不是产品入口）。
//!
//! 为什么它留在仓库里：spike 的产出是**证据**，证据必须能被重跑。
//! 结论与全部实测输出见 `docs/plans/2026-10-16-m3-port-transport-spike.md`；
//! 它只做 M1 壳没有的三件事：
//! 1. **在 Rust 侧建窗口**并挂 `initialization_script`
//!    （`tauri.conf.json` 的 `WindowConfig` 没有该字段，见 `tauri-utils/src/config.rs`）；
//! 2. 把页面 `port2` 的字节与 Node Host（`test/fixtures/stdioChannelHost.mts`，真 `ChannelServer`）
//!    的 stdio 长度前缀帧对接：页面 → 壳用 raw body 的 invoke（`InvokeBody::Raw`），
//!    壳 → 页面用长轮询命令返回 `tauri::ipc::Response`（raw bytes）。
//! 3. 三个模式复现三种现场（见 `Mode`）。
//!
//! 运行（详见 spike 文档 §3.2）——**顺序不能倒**：`frontendDist` 由
//! `tauri::generate_context!()` 在**编译期**内嵌，所以要先把页面构建出来再编 Rust：
//! ```text
//! cd apps/desktop-tauri && pnpm exec vite build --config test/harness/port-transport/vite.config.ts
//! node scripts/rust-cargo.ts build --release -p mode-tauri --features port-transport-harness \
//!   --bin PortTransportHarness
//! ./target/release/PortTransportHarness.exe [local|no-wrap|remote]
//! ```
//! 退出码：0 = 往返成功；3 = 页面报了失败结果（`no-wrap` 的预期码）；4 = 看门狗超时
//! （`remote` 的预期码：ACL 拒绝使页面无法回报，证据在 dev server 的 `SPIKE_PAGE_LOG` 里）。

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::State;

const STARTUP_ID: &str = "harness-startup-1";
const WATCHDOG: Duration = Duration::from_secs(40);

/// 三种要复现的现场。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 正确形态：入方向包 `Uint8Array`，窗口加载本地内容 ⇒ 期望 `status: "ok"`，退出 0。
    Local,
    /// 失败形态：入方向直接投 raw `ArrayBuffer` ⇒ `MessagePortProtocol` 静默丢弃，
    /// `Initialize` 永不达、RPC 挂到超时 ⇒ 期望退出 3。
    NoWrap,
    /// 失败形态：窗口指向 http:// 来源（非本地内容）⇒ app 命令被 ACL 拒 ⇒ 期望退出 4。
    Remote,
}

impl Mode {
    fn parse(raw: Option<&String>) -> Self {
        match raw.map(String::as_str) {
            None | Some("local") => Self::Local,
            Some("no-wrap") => Self::NoWrap,
            Some("remote") => Self::Remote,
            Some(other) => {
                eprintln!("SPIKE_INFO unknown_mode={other}（可用：local / no-wrap / remote）");
                std::process::exit(2);
            }
        }
    }

    fn wrap_port_message(self) -> bool {
        !matches!(self, Self::NoWrap)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::NoWrap => "no-wrap",
            Self::Remote => "remote",
        }
    }
}

struct HostPipe {
    child: Mutex<Option<Child>>,
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<ChildStdout>,
}

#[derive(Clone)]
struct HostHandle(Arc<HostPipe>);

fn write_frame(stdin: &mut ChildStdin, bytes: &[u8]) -> std::io::Result<()> {
    let mut header = [0u8; 4];
    header.copy_from_slice(&(bytes.len() as u32).to_be_bytes());
    stdin.write_all(&header)?;
    stdin.write_all(bytes)?;
    stdin.flush()
}

fn read_frame(stdout: &mut ChildStdout) -> std::io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stdout.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    let mut body = vec![0u8; len];
    stdout.read_exact(&mut body)?;
    Ok(body)
}

/// 页面 → 壳：整包 raw 字节（页面侧 `invoke(cmd, ArrayBuffer|TypedArray)` → `octet-stream`）。
#[tauri::command]
fn spike_host_send(handle: State<'_, HostHandle>, request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        other => return Err(format!("spike_host_send 期望 raw body，实收 {other:?}")),
    };
    let mut stdin = handle.0.stdin.lock().map_err(|e| e.to_string())?;
    write_frame(&mut stdin, &bytes).map_err(|e| format!("写 Host 帧失败：{e}"))
}

/// 壳 → 页面：长轮询读一帧，按 raw bytes 返回（页面侧拿到 `ArrayBuffer`）。
#[tauri::command]
async fn spike_host_recv(handle: State<'_, HostHandle>) -> Result<tauri::ipc::Response, String> {
    let host = handle.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut stdout = host.stdout.lock().map_err(|e| e.to_string())?;
        read_frame(&mut stdout).map_err(|e| format!("读 Host 帧失败：{e}"))
    })
    .await
    .map_err(|e| e.to_string())?
    .map(tauri::ipc::Response::new)
}

struct Watchdog(Arc<AtomicBool>);

/// 页面报结果：打到 stdout，并只在**最终结果**（带 `"status":`）时结束进程，
/// 让 harness 可以无人值守跑（中间编排日志如 `delivered port+state` 不结束进程）。
#[tauri::command]
fn spike_report(app: tauri::AppHandle, reported: State<'_, Watchdog>, line: String) {
    println!("SPIKE_REPORT {line}");
    let _ = std::io::stdout().flush();
    if !line.contains("\"status\":") {
        // 编排期的中间日志（例如 delivered port+state）不是结果：既不能结束进程，
        // 也不能喂给看门狗——否则看门狗以为已经拿到结果，进程挂住也没人报。
        return;
    }
    reported.0.store(true, Ordering::SeqCst);
    let code = if line.contains("\"status\":\"ok\"") { 0 } else { 3 };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        app.exit(code);
    });
}

fn main() {
    let mode = Mode::parse(std::env::args().nth(1).as_ref());
    println!("SPIKE_INFO mode={}", mode.name());

    // 不用 canonicalize()：Windows 上它会给出 `\\?\D:\...` 扩展前缀，Node 解析不了
    // （实测 `Error: EISDIR: illegal operation on a directory, lstat 'D:'`，见 spike 文档 §4.6）。
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let host_script = repo_root.join("apps/desktop-tauri/test/fixtures/stdioChannelHost.mts");

    let mut child = Command::new("node")
        .args(["--import", "tsx"])
        .arg(&host_script)
        .current_dir(&repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("拉起 Host 替身（需要 PATH 里有 node，且依赖已安装）");
    let stdin = child.stdin.take().expect("host stdin");
    let stdout = child.stdout.take().expect("host stdout");
    println!("SPIKE_INFO host_spawned pid={}", child.id());

    let handle = HostHandle(Arc::new(HostPipe {
        child: Mutex::new(Some(child)),
        stdin: Mutex::new(stdin),
        stdout: Mutex::new(stdout),
    }));

    let reported = Arc::new(AtomicBool::new(false));
    let script: String =
        include_str!("../../../test/harness/port-transport/frontend/injected-bridge.ts")
        .replace("__SPIKE_STARTUP_ID__", STARTUP_ID)
        .replace(
            "__SPIKE_WRAP_PORT_MESSAGE__",
            if mode.wrap_port_message() { "true" } else { "false" },
        );

    let watchdog_flag = reported.clone();
    let watchdog_handle = handle.clone();

    tauri::Builder::default()
        .manage(handle)
        .manage(Watchdog(reported))
        .invoke_handler(tauri::generate_handler![
            spike_host_send,
            spike_host_recv,
            spike_report
        ])
        .setup(move |app| {
            let url = match mode {
                // 本地内容（asset 协议）= 产品形态：Task 3 的 renderer 也由 frontendDist 提供。
                Mode::Local | Mode::NoWrap => tauri::WebviewUrl::App(std::path::PathBuf::from(
                    "harness/port-transport/index.html",
                )),
                // 远端 http 来源：本仓未配 devUrl，页面不是本地内容 ⇒ app 命令被 ACL 拒（复现现场）。
                Mode::Remote => tauri::WebviewUrl::External(
                    "http://127.0.0.1:5176/".parse().expect("harness 远端 URL"),
                ),
            };
            tauri::WebviewWindowBuilder::new(app, "port-transport-harness", url)
                .title("M3 Task 0 · port transport harness")
                .inner_size(1100.0, 700.0)
                .initialization_script(script)
                .build()?;

            std::thread::spawn(move || {
                std::thread::sleep(WATCHDOG);
                if watchdog_flag.load(Ordering::SeqCst) {
                    return;
                }
                // 保持 ASCII：这行会被粘进 spike 文档当原始输出，别让控制台代码页把它变成乱码。
                println!(
                    "SPIKE_TIMEOUT no_report_within_{}s (remote mode expects this: ACL denial means the page cannot report)",
                    WATCHDOG.as_secs()
                );
                let _ = std::io::stdout().flush();
                if let Some(child) = watchdog_handle.0.child.lock().ok().and_then(|mut c| c.take()) {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &child.id().to_string(), "/T", "/F"])
                        .status();
                }
                std::process::exit(4);
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("harness 壳启动失败");
}
