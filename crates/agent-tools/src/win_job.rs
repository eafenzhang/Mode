//! Windows Job Object：把 Bash 子进程挂进 **kill-on-close** 的作业对象。
//!
//! 背景（残余清单）：agent 进程被强杀时，`taskkill /T`/优雅退出都来不及执行，
//! Bash 的孙子进程会成为孤儿。作业对象句柄随 agent 进程死亡由 OS 关闭，
//! 内核会直接终止整个子树——这是唯一在「强杀」场景下仍然成立的防线。
//! 非 Windows 平台没有此模块（Unix 上进程组信号由 `kill_tree` 覆盖）。

#![cfg(windows)]

use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// 一个已挂接子进程的作业对象句柄；Drop 时 CloseHandle。
///
/// 句柄关闭（含 agent 进程被强杀时由 OS 代收）会终止作业内全部进程；
/// 子进程正常退出后关闭句柄是无操作的。
pub struct JobHandle(HANDLE);

// HANDLE 只是内核句柄整数；Drop 只调 CloseHandle，跨线程移动安全。
unsafe impl Send for JobHandle {}

impl std::fmt::Debug for JobHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不打印句柄值（内部诊断面不暴露内核句柄）。
        formatter.write_str("JobHandle")
    }
}

impl JobHandle {
    /// 创建作业对象、设置 kill-on-close，并把子进程挂进去。
    ///
    /// 任一步失败返回 `None`（调用方降级为无作业保护，不阻断工具调用）。
    pub fn assign(child: &std::process::Child) -> Option<JobHandle> {
        // SAFETY：CreateJobObjectW/SetInformationJobObject/AssignProcessToJobObject
        // 均按 Win32 文档传参；失败路径立即 CloseHandle，成功路径句柄由 Self 接管。
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(job);
                return None;
            }
            if AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(JobHandle(job))
        }
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        // SAFETY：句柄由 Self 持有，Drop 时唯一关闭。
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// kill-on-close 生效：Drop 句柄后，仍在运行的长命令子进程被内核终止
    /// （不用等它自然结束）——这就是「agent 被强杀 → 子树回收」的等价验证。
    #[test]
    fn dropping_the_handle_kills_the_subtree() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 30 127.0.0.1"])
            .spawn()
            .expect("spawn long command");
        let Some(job) = JobHandle::assign(&child) else {
            child.kill().expect("kill probe");
            let _ = child.wait();
            eprintln!("作业对象不可用（环境限制），跳过 kill-on-close 断言");
            return;
        };
        drop(job);
        let started = std::time::Instant::now();
        let status = child.wait().expect("wait after job close");
        let elapsed = started.elapsed();
        eprintln!("exit={status:?} elapsed={elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "30s 的子进程应在句柄关闭后被内核终止（kill-on-close），实际耗时 {elapsed:?}"
        );
    }
}
