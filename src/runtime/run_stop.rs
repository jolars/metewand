//! Process-wide stop request for the Unix CLI coordinator.

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
static REQUESTED: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
static ENABLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
pub fn install() -> std::io::Result<()> {
    extern "C" fn request(_: libc::c_int) {
        REQUESTED.store(true, Ordering::Relaxed);
    }

    // SAFETY: The action and mask are initialized before registration. The
    // handler only performs a lock-free atomic store.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = request as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn enabled() -> bool {
    #[cfg(unix)]
    {
        ENABLED.load(Ordering::Relaxed)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

pub(crate) fn requested() -> bool {
    #[cfg(unix)]
    {
        REQUESTED.load(Ordering::Relaxed)
    }
    #[cfg(not(unix))]
    {
        false
    }
}
