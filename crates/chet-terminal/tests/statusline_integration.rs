//! Integration tests for the StatusLine + REPL startup sequence.
//!
//! Uses the vt100 terminal emulator to verify that the full startup flow
//! (install → suspend → banner → prompt) produces correct screen output.
//! This catches bugs like DECSTBM cursor homing overwriting the banner.
//!
//! Run with: `cargo test -p chet-terminal --test statusline_integration`

use chet_terminal::{StatusLine, StatusLineData};
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn test_data() -> StatusLineData {
    StatusLineData {
        model: "claude-sonnet-4-5-20250929".to_string(),
        session_id: "a1b2c3d4".to_string(),
        ..StatusLineData::default()
    }
}

fn screen_from(bytes: &[u8], rows: u16, cols: u16) -> vt100::Parser {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(bytes);
    parser
}

/// Simulates the exact REPL startup sequence and verifies screen output.
/// This is the flow that caused the original cursor-homing bug.
#[test]
fn repl_startup_banner_not_overwritten() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = CaptureWriter(buf.clone());

    let mut sl = StatusLine::new_with_writer(test_data(), 80, 24, Box::new(writer.clone()));

    // Step 1: Install + suspend BEFORE banner (matches REPL)
    sl.install();
    sl.suspend();

    // Step 2: Banner prints at current cursor position
    let _ = write!(
        writer.clone(),
        "chet v0.3.3 (model: claude-sonnet-4-5-20250929, session: a1b2c3d4)\r\n"
    );
    let _ = write!(
        writer.clone(),
        "Type your message. Press Ctrl+D to exit.\r\n\r\n"
    );

    // Step 3: Prompt drawn at current position (no suspend — first iteration)
    let _ = write!(writer.clone(), "> ");

    // Verify screen state
    let parser = screen_from(&buf.lock().unwrap(), 24, 80);
    let screen = parser.screen();

    let row0 = screen.contents_between(0, 0, 1, 80);
    assert!(
        row0.starts_with("chet v0.3.3"),
        "row 0 should have banner, got: {:?}",
        row0.trim()
    );

    let row1 = screen.contents_between(1, 0, 2, 80);
    assert!(
        row1.contains("Type your message"),
        "row 1 should have instructions, got: {:?}",
        row1.trim()
    );

    let row3 = screen.contents_between(3, 0, 4, 80);
    assert!(
        row3.starts_with("> "),
        "row 3 should have prompt, got: {:?}",
        row3.trim()
    );

    let (cursor_row, cursor_col) = screen.cursor_position();
    assert_eq!(cursor_row, 3, "cursor should be on row 3 (prompt row)");
    assert_eq!(cursor_col, 2, "cursor should be at col 2 (after '> ')");
}

/// Simulates a full REPL cycle: startup → input → agent response → next prompt.
/// Verifies cursor tracking works across resume/suspend cycles.
#[test]
fn repl_full_cycle_cursor_tracking() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = CaptureWriter(buf.clone());

    // Use 120 cols to avoid status line autowrap at the terminal edge,
    // which can cause a scroll on some VT implementations (seen on Windows CI).
    let mut sl = StatusLine::new_with_writer(test_data(), 120, 24, Box::new(writer.clone()));

    // Startup: install + suspend + banner
    sl.install();
    sl.suspend();
    let _ = write!(writer.clone(), "chet v0.3.3 (model: test)\r\n");
    let _ = write!(writer.clone(), "Type your message.\r\n\r\n");

    // First prompt (no suspend needed — already suspended)
    let _ = write!(writer.clone(), "> hello world\r\n");

    // Resume for agent execution (cursor now at row 4)
    sl.set_cursor_row(4);
    sl.resume();

    // Agent output
    let _ = write!(writer.clone(), "Here is my response.\r\n");
    let _ = write!(writer.clone(), "It has multiple lines.\r\n");

    // Suspend for next prompt (cursor at row 6)
    sl.set_cursor_row(6);
    sl.suspend();

    // Second prompt
    let _ = write!(writer.clone(), "> ");

    // Verify final screen state — check content ordering rather than exact rows.
    let parser = screen_from(&buf.lock().unwrap(), 24, 120);
    let screen = parser.screen();
    let full = screen.contents();

    assert!(
        full.contains("chet v0.3.3"),
        "banner should survive full cycle"
    );
    assert!(
        full.contains("Here is my response"),
        "agent response should be visible"
    );

    // Verify ordering: banner before agent output before prompt
    let banner_pos = full.find("chet v0.3.3").unwrap();
    let response_pos = full.find("Here is my response").unwrap();
    let prompt_pos = full.rfind("> ").unwrap();
    assert!(
        banner_pos < response_pos && response_pos < prompt_pos,
        "content should be in order: banner < response < prompt"
    );
}

/// Verifies that stale terminal content (e.g., old shell history) is
/// cleared when the status line suspends. This was the root cause of
/// old prompts and commands remaining visible after chet started.
#[test]
fn repl_startup_clears_stale_content() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = CaptureWriter(buf.clone());

    // Simulate pre-existing terminal content (shell history)
    let _ = write!(writer.clone(), "$ ls -la\r\n");
    let _ = write!(writer.clone(), "total 42\r\n");
    let _ = write!(writer.clone(), "drwxr-xr-x 5 user user 4096 ...\r\n");
    let _ = write!(writer.clone(), "$ vim foo.rs\r\n");
    let _ = write!(writer.clone(), "$ ./target/release/chet\r\n");

    let mut sl = StatusLine::new_with_writer(test_data(), 80, 24, Box::new(writer.clone()));

    // Install + suspend (with \x1b[J clearing stale content)
    sl.install();
    sl.suspend();

    // Banner
    let _ = write!(writer.clone(), "chet v0.3.3\r\n");
    let _ = write!(writer.clone(), "Type your message.\r\n\r\n");
    let _ = write!(writer.clone(), "> ");

    let parser = screen_from(&buf.lock().unwrap(), 24, 80);
    let screen = parser.screen();

    // Rows below the prompt should be empty (stale content cleared)
    let row4 = screen.contents_between(4, 0, 5, 80);
    assert!(
        row4.trim().is_empty(),
        "row below prompt should be empty (stale content cleared), got: {:?}",
        row4.trim()
    );

    // The old shell content should NOT appear after the prompt
    let full = screen.contents();
    assert!(
        !full.contains("vim foo.rs"),
        "old shell commands should be cleared"
    );
}

/// Verifies behavior on very small terminals (height < 3) where the
/// status line is disabled entirely.
#[test]
fn repl_startup_tiny_terminal_no_crash() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = CaptureWriter(buf.clone());

    // 2-row terminal: too small for status line
    let mut sl = StatusLine::new_with_writer(test_data(), 80, 2, Box::new(writer.clone()));

    sl.install(); // should be no-op (height < 3)
    sl.suspend(); // should be no-op (not installed)

    let _ = write!(writer.clone(), "chet v0.3.3\r\n> ");

    let parser = screen_from(&buf.lock().unwrap(), 2, 80);
    let screen = parser.screen();

    let row0 = screen.contents_between(0, 0, 1, 80);
    assert!(
        row0.starts_with("chet"),
        "banner should work without status line, got: {:?}",
        row0.trim()
    );
}
