//! Braille spinner for visual feedback during API requests and tool execution.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

/// Braille animation frames.
const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Frame interval in milliseconds.
const FRAME_MS: u64 = 80;

/// A terminal spinner that renders braille animation on stderr.
///
/// The spinner runs as a background tokio task. Control it via `set_active()`
/// and `set_message()`. Call `stop()` to clean up.
pub struct Spinner {
    active: Arc<AtomicBool>,
    message: Arc<Mutex<String>>,
    handle: JoinHandle<()>,
}

impl Spinner {
    /// Create and start a new spinner with the given initial message.
    pub fn new(message: &str) -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let msg = Arc::new(Mutex::new(message.to_string()));

        let active_clone = active.clone();
        let msg_clone = msg.clone();

        let handle = tokio::spawn(async move {
            let mut frame_idx = 0;
            loop {
                if active_clone.load(Ordering::Relaxed) {
                    let msg_text = msg_clone.lock().unwrap().clone();
                    let frame = FRAMES[frame_idx % FRAMES.len()];
                    let _ = write!(std::io::stderr(), "\r  {frame} {msg_text}");
                    let _ = std::io::stderr().flush();
                    frame_idx += 1;
                }
                tokio::time::sleep(std::time::Duration::from_millis(FRAME_MS)).await;
            }
        });

        Self {
            active,
            message: msg,
            handle,
        }
    }

    /// Enable or disable the spinner animation.
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Relaxed);
    }

    /// Update the spinner message.
    pub fn set_message(&self, msg: &str) {
        *self.message.lock().unwrap() = msg.to_string();
    }

    /// Stop the spinner, abort the background task, and clear the line.
    pub async fn stop(self) {
        self.active.store(false, Ordering::Relaxed);
        self.handle.abort();
        let _ = self.handle.await;
        clear_line();
    }
}

/// Clear the current spinner line on stderr.
///
/// Safe to call from sync context.
pub fn clear_line() {
    let _ = write!(std::io::stderr(), "\r\x1b[2K");
    let _ = std::io::stderr().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_send<T: Send>() {}
    fn is_sync<T: Sync>() {}

    #[test]
    fn spinner_is_send() {
        is_send::<Spinner>();
    }

    #[test]
    fn spinner_is_sync() {
        is_sync::<Spinner>();
    }

    #[test]
    fn clear_line_compiles_in_sync() {
        // Just verify it can be called from a non-async context
        clear_line();
    }

    #[test]
    fn frames_has_entries() {
        assert!(FRAMES.len() >= 2);
    }

    #[tokio::test]
    async fn create_and_stop_no_panic() {
        let spinner = Spinner::new("Testing...");
        spinner.set_active(false);
        spinner.stop().await;
    }

    #[tokio::test]
    async fn set_active_and_message_no_panic() {
        let spinner = Spinner::new("init");
        spinner.set_active(false);
        spinner.set_message("changed");
        spinner.set_active(true);
        spinner.set_active(false);
        spinner.stop().await;
    }
}
