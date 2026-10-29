//! Stops the server when the console window hosting the client is closed.
//!
//! Windows gives a console process a few seconds after `CTRL_CLOSE_EVENT`, so the
//! handler asks the server to stop directly instead of waiting for the client
//! loop. The server saves the session during shutdown, which lets the next start
//! restore the layout and resume supported agents.

use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Console::{
    SetConsoleCtrlHandler, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
};

unsafe extern "system" fn handle_console_event(event: u32) -> i32 {
    match event {
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            if let Err(err) = crate::session::stop_active_server() {
                tracing::warn!(%err, "failed to stop server on window close");
            }
            TRUE
        }
        _ => FALSE,
    }
}

pub(super) fn install_stop_server_handler() {
    // SAFETY: the handler is a plain function that lives for the whole process.
    if unsafe { SetConsoleCtrlHandler(Some(handle_console_event), TRUE) } == 0 {
        tracing::warn!(
            error = %std::io::Error::last_os_error(),
            "failed to install window close handler"
        );
    }
}
