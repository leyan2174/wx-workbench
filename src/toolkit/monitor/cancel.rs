//! 控制台回调只置原子标志；RAII 移除本次回调，不接管 daemon 生命周期。
use anyhow::Result;
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};

#[derive(Clone, Default)]
pub struct Cancellation { requested: Arc<AtomicBool>, console: bool }
impl Cancellation {
    pub fn new() -> Self { Self::default() }
    pub fn cancel(&self) { self.requested.store(true, Ordering::Release); }
    pub fn is_cancelled(&self) -> bool {
        if self.requested.load(Ordering::Acquire) { return true; }
        #[cfg(windows)]
        if self.console && CONSOLE_STOP.load(Ordering::Acquire) { return true; }
        false
    }
    pub async fn cancelled(&self) {
        while !self.is_cancelled() { tokio::time::sleep(Duration::from_millis(25)).await; }
    }
}

pub struct ConsoleCancellation { token: Cancellation }
impl ConsoleCancellation {
    pub fn install() -> Result<Self> {
        #[cfg(windows)]
        {
            use windows::Win32::System::Console::SetConsoleCtrlHandler;
            anyhow::ensure!(CONSOLE_ACTIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok(), "同一进程只能安装一个监控控制台处理器");
            CONSOLE_STOP.store(false, Ordering::Release);
            if let Err(error) = unsafe { SetConsoleCtrlHandler(Some(control_handler), true) } {
                CONSOLE_ACTIVE.store(false, Ordering::Release);
                return Err(error.into());
            }
            Ok(Self { token: Cancellation { requested: Arc::new(AtomicBool::new(false)), console:true } })
        }
        #[cfg(not(windows))]
        { anyhow::bail!("控制台监控仅支持 Windows；嵌入调用可传入 Cancellation"); }
    }
    pub fn token(&self) -> Cancellation { self.token.clone() }
}
impl Drop for ConsoleCancellation {
    fn drop(&mut self) {
        self.token.cancel();
        #[cfg(windows)]
        {
            let removed = unsafe { windows::Win32::System::Console::SetConsoleCtrlHandler(Some(control_handler), false) };
            // 移除失败时不允许再次安装，避免同一进程出现叠加回调。
            if removed.is_ok() { CONSOLE_ACTIVE.store(false, Ordering::Release); }
        }
    }
}

#[cfg(windows)]
static CONSOLE_STOP: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static CONSOLE_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
unsafe extern "system" fn control_handler(kind: u32) -> windows::Win32::Foundation::BOOL {
    use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    if kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT {
        CONSOLE_STOP.store(true, Ordering::Release);
        windows::Win32::Foundation::BOOL(1)
    } else { windows::Win32::Foundation::BOOL(0) }
}
