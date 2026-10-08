//! 进程级单例 UIA STA 命令线程。
//!
//! UIA 客户端对象有 COM 单元亲和：全部 UIA 调用在同一条 STA 线程上串行执行，
//! 调用方（任意线程，含 napi 主线程与测试线程）`send` 命令 + `recv_timeout(30s)`。
//!
//! - 首次调用惰性 spawn；线程入口 `CoInitializeEx(COINIT_APARTMENTTHREADED)`。
//! - 单请求 30s 超时 → `timeout`。
//! - 命令处理 panic 按命令区分映射：**observe** panic → 当次 `timeout`（brief 语义），且丢弃
//!   可能处于不一致状态的 `IUIAutomation` 供下次重建；**capture** panic → `internal`，不丢弃
//!   automation（截图不读写 UIA 缓存，无一致化问题）——两者线程本身均继续服务。
//! - 线程死亡（send/disconnect 失败）→ `timeout` 并清掉单例 sender，下次调用重新 spawn。
use crate::capture::{self, CaptureResultNapi};
use crate::error::{AxError, AxResult};
use crate::observe;
use crate::ObserveResultNapi;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;
use windows::Win32::System::Com::{
  CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
  COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};

/// 单请求超时（spec「单请求超时 → timeout」）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// 发往 STA 线程的命令。GDI 截图（Task 4）与 observe 串行共线程；Task 5 的 perform 在此追加变体。
enum Cmd {
  Observe {
    window_id: u32,
    max_elements: u32,
    reply: Sender<AxResult<ObserveResultNapi>>,
  },
  Capture {
    window_id: Option<u32>,
    region: Option<Vec<i32>>,
    full_screen: bool,
    reply: Sender<AxResult<CaptureResultNapi>>,
  },
}

/// 单例槽：(世代号, 命令 sender)；Some 即「线程已 spawn」。
type CellSlot = Option<(u64, Sender<Cmd>)>;

/// 进程级单例通道。std mpsc 没有 channel 同一性比较（same_channel 属 crossbeam），
/// 用世代号识别「失败的这条 sender 是否还是当前单例」，避免清掉别人刚重建的新线程。
static CELL: LazyLock<Mutex<CellSlot>> = LazyLock::new(|| Mutex::new(None));
static SPAWN_GEN: AtomicU64 = AtomicU64::new(0);

/// 锁只在 clone/spawn 期间持有、锁内无用户代码，poison 兜底取回即可。
fn lock_cell() -> MutexGuard<'static, CellSlot> {
  CELL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 取（或首次创建）单例 STA 线程的 (世代号, 命令 sender)。
fn sender() -> AxResult<(u64, Sender<Cmd>)> {
  let mut cell = lock_cell();
  if let Some((gen, tx)) = cell.as_ref() {
    return Ok((*gen, tx.clone()));
  }
  let (tx, rx) = mpsc::channel();
  std::thread::Builder::new()
    .name("mode-cua-uia-sta".into())
    .spawn(move || sta_loop(rx))
    .map_err(|e| AxError::internal(format!("spawn UIA STA thread failed: {e}")))?;
  let gen = SPAWN_GEN.fetch_add(1, Ordering::Relaxed) + 1;
  *cell = Some((gen, tx.clone()));
  Ok((gen, tx))
}

/// 线程已死时按世代号清掉单例（世代不符 = 已有新线程，不动它），下次调用重新 spawn。
fn discard(gen: u64) {
  let mut cell = lock_cell();
  if cell.as_ref().is_some_and(|(g, _)| *g == gen) {
    *cell = None;
  }
}

/// 通用命令投递：send + 30s 等待；超时/线程死亡均按契约映射 `timeout`。
/// `make_cmd` 把 reply 通道装进具体变体——observe/capture 共用这一条投递路径。
fn submit<T>(make_cmd: impl FnOnce(Sender<AxResult<T>>) -> Cmd) -> AxResult<T> {
  let (gen, tx) = sender()?;
  let (reply_tx, reply_rx) = mpsc::channel();
  if tx.send(make_cmd(reply_tx)).is_err() {
    discard(gen);
    return Err(AxError::new("timeout", "UIA STA 线程已退出（send 失败）"));
  }
  match reply_rx.recv_timeout(REQUEST_TIMEOUT) {
    Ok(result) => result,
    Err(RecvTimeoutError::Timeout) => Err(AxError::new("timeout", "UIA 命令超时（30s）")),
    // reply 通道断开 = 线程在回复前死亡（panic 逃逸出命令边界等）。
    Err(RecvTimeoutError::Disconnected) => {
      discard(gen);
      Err(AxError::new("timeout", "UIA STA 线程已退出（回复中断）"))
    }
  }
}

/// observe 命令投递（语义见 `submit`）。
pub(crate) fn submit_observe(window_id: u32, max_elements: u32) -> AxResult<ObserveResultNapi> {
  submit(move |reply| Cmd::Observe {
    window_id,
    max_elements,
    reply,
  })
}

/// capture 命令投递（语义见 `submit`）：GDI 截图与 observe 在同一 STA 线程串行。
pub(crate) fn submit_capture(
  window_id: Option<u32>,
  region: Option<Vec<i32>>,
  full_screen: bool,
) -> AxResult<CaptureResultNapi> {
  submit(move |reply| Cmd::Capture {
    window_id,
    region,
    full_screen,
    reply,
  })
}

/// STA 线程主体：初始化 COM → 串行消费命令。
fn sta_loop(rx: Receiver<Cmd>) {
  // 全新线程上必为 S_OK；即便失败也继续——后续 UIA 调用会自带错误并被映射。
  unsafe {
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
  }
  // IUIAutomation 线程本地缓存（懒建）；命令 panic 后丢弃重建。
  let mut automation: Option<IUIAutomation> = None;
  while let Ok(cmd) = rx.recv() {
    match cmd {
      Cmd::Observe {
        window_id,
        max_elements,
        reply,
      } => {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
          let auto = with_automation(&mut automation)?;
          observe::run_observe(auto, window_id, max_elements)
        }));
        let outcome = match outcome {
          Ok(result) => result,
          Err(_) => {
            // panic 只让当次调用失败（timeout）；automation 状态不可信，丢弃待重建。
            automation = None;
            Err(AxError::new("timeout", "UIA STA 命令处理 panic"))
          }
        };
        // 接收方可能已超时放弃：晚到的回复投递失败只能丢弃。
        let _ = reply.send(outcome);
      }
      Cmd::Capture {
        window_id,
        region,
        full_screen,
        reply,
      } => {
        // 截图 panic 不触碰 automation 缓存（不读不写），故不丢弃——与 observe 分开处理。
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
          capture::run_capture(window_id, region, full_screen)
        }));
        let outcome = match outcome {
          Ok(result) => result,
          Err(_) => Err(AxError::internal("capture 命令处理 panic")),
        };
        let _ = reply.send(outcome);
      }
    }
  }
  unsafe {
    CoUninitialize();
  }
}

/// 取（或首次创建）本线程的 `IUIAutomation`；创建失败逐次返回 internal。
fn with_automation(slot: &mut Option<IUIAutomation>) -> AxResult<&IUIAutomation> {
  if slot.is_none() {
    let created: IUIAutomation =
      unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| AxError::internal(format!("CoCreateInstance(CUIAutomation) failed: {e}")))?;
    *slot = Some(created);
  }
  match slot.as_ref() {
    Some(auto) => Ok(auto),
    // 上一分支已写入，失败路径提前 return——防御性兜底，不 unwrap。
    None => Err(AxError::internal("CUIAutomation 缓存丢失")),
  }
}
