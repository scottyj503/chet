//! Persistent status line pinned to the terminal bottom row.
//!
//! Uses DECSTBM (Set Top and Bottom Margins) to restrict scrolling to rows
//! 1 through height-1, leaving the bottom row for the status bar. All existing
//! stdout/stderr output naturally stays within the scroll region.

use chet_types::Effort;
use std::io::Write;

/// Plain data for the status line segments.
pub struct StatusLineData {
    pub model: String,
    pub session_id: String,
    pub context_tokens_k: f64,
    pub context_window_k: f64,
    pub context_percent: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub effort: Option<Effort>,
    pub plan_mode: bool,
    pub active_tool: Option<String>,
}

impl Default for StatusLineData {
    fn default() -> Self {
        Self {
            model: String::new(),
            session_id: String::new(),
            context_tokens_k: 0.0,
            context_window_k: 0.0,
            context_percent: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            effort: None,
            plan_mode: false,
            active_tool: None,
        }
    }
}

/// Terminal status bar manager using DECSTBM scroll regions.
pub struct StatusLine {
    data: StatusLineData,
    terminal_height: u16,
    terminal_width: u16,
    installed: bool,
    writer: Box<dyn Write + Send>,
    /// Tracked cursor row — used to restore position after DECSTBM,
    /// which homes the cursor on many terminals and clobbers save buffers.
    cursor_row: u16,
}

impl StatusLine {
    /// Create a new status line (does not install it yet).
    pub fn new(data: StatusLineData) -> Self {
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        Self {
            data,
            terminal_width: w,
            terminal_height: h,
            installed: false,
            writer: Box::new(std::io::stderr()),
            cursor_row: 0,
        }
    }

    /// Create a status line with explicit dimensions and writer (for testing).
    pub fn new_with_writer(
        data: StatusLineData,
        width: u16,
        height: u16,
        writer: Box<dyn Write + Send>,
    ) -> Self {
        Self {
            data,
            terminal_width: width,
            terminal_height: height,
            installed: false,
            writer,
            cursor_row: 0,
        }
    }

    /// Set the tracked cursor row (call before install to sync with actual position).
    pub fn set_cursor_row(&mut self, row: u16) {
        self.cursor_row = row;
    }

    /// Install the scroll region and draw the initial status line.
    /// No-op if terminal height < 3.
    pub fn install(&mut self) {
        if self.terminal_height < 3 {
            return;
        }
        self.set_scroll_region();
        self.draw();
        self.installed = true;
    }

    /// Replace data and redraw.
    pub fn update(&mut self, data: StatusLineData) {
        self.data = data;
        if self.installed {
            self.draw();
        }
    }

    /// Update a single field and redraw.
    pub fn update_field(&mut self, f: impl FnOnce(&mut StatusLineData)) {
        f(&mut self.data);
        if self.installed {
            self.draw();
        }
    }

    /// Suspend the status line: reset scroll region to full screen, clear bottom row.
    /// Used before the line editor enters raw mode.
    pub fn suspend(&mut self) {
        if !self.installed {
            return;
        }
        // Reset scroll region, restore cursor via explicit CUP, clear below
        let row = self.cursor_row;
        let _ = write!(self.writer, "\x1b[r\x1b[{};1H\x1b[J", row + 1);
        let _ = self.writer.flush();
    }

    /// Resume the status line after the line editor returns.
    pub fn resume(&mut self) {
        if !self.installed {
            return;
        }
        // Re-check terminal size in case it changed
        if let Ok((w, h)) = crossterm::terminal::size() {
            self.terminal_width = w;
            self.terminal_height = h;
        }
        if self.terminal_height < 3 {
            return;
        }
        self.set_scroll_region();
        self.draw();
    }

    /// Handle terminal resize.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal_width = width;
        self.terminal_height = height;
        if !self.installed {
            return;
        }
        if height < 3 {
            // Too small — reset scroll region, restore cursor
            let row = self.cursor_row;
            let _ = write!(self.writer, "\x1b[r\x1b[{};1H", row + 1);
            let _ = self.writer.flush();
            return;
        }
        self.set_scroll_region();
        self.draw();
    }

    /// Tear down: reset scroll region, clear bottom row.
    pub fn teardown(&mut self) {
        if !self.installed {
            return;
        }
        self.installed = false;
        // Reset scroll region, restore cursor
        let row = self.cursor_row;
        let _ = write!(self.writer, "\x1b[r\x1b[{};1H", row + 1);
        // Clear the bottom row
        let _ = write!(
            self.writer,
            "\x1b7\x1b[{};1H\x1b[2K\x1b8",
            self.terminal_height
        );
        let _ = self.writer.flush();
    }

    /// Set DECSTBM scroll region to rows 1..height-1.
    /// Uses explicit CUP to restore cursor position because DECSTBM homes the
    /// cursor on many terminals and clobbers both DEC and SCO save buffers.
    fn set_scroll_region(&mut self) {
        let row = self.cursor_row;
        let _ = write!(
            self.writer,
            "\x1b[1;{}r\x1b[{};1H",
            self.terminal_height - 1,
            row + 1
        );
        let _ = self.writer.flush();
    }

    /// Draw the status line on the bottom row.
    fn draw(&mut self) {
        let rendered = self.render();
        // Save cursor, move to bottom row, clear line, write, restore cursor
        let _ = write!(
            self.writer,
            "\x1b7\x1b[{};1H\x1b[2K{}\x1b8",
            self.terminal_height, rendered
        );
        let _ = self.writer.flush();
    }

    /// Render the status line content with dim+reverse styling.
    fn render(&self) -> String {
        let content = render_segments(&self.data);
        let width = self.terminal_width as usize;

        // Truncate or pad to fill terminal width
        let display_len = display_width(&content);
        let padded = if display_len >= width {
            truncate_to_width(&content, width)
        } else {
            let padding = width - display_len;
            format!("{}{}", content, " ".repeat(padding))
        };

        // Apply dim + reverse video styling
        format!("\x1b[2;7m{}\x1b[0m", padded)
    }
}

impl Drop for StatusLine {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// Render the status line segments joined by ` | `.
fn render_segments(data: &StatusLineData) -> String {
    let mut segments = Vec::with_capacity(6);

    // Model or active tool
    match &data.active_tool {
        Some(tool) => {
            if tool.starts_with("mcp__") {
                // Format mcp__server__tool as "mcp: server>tool"
                let parts: Vec<&str> = tool.splitn(3, "__").collect();
                if parts.len() == 3 {
                    segments.push(format!("mcp: {}>{}", parts[1], parts[2]));
                } else {
                    segments.push(format!("running: {tool}"));
                }
            } else {
                segments.push(format!("running: {tool}"));
            }
        }
        None => {
            segments.push(shorten_model_name(&data.model));
        }
    }

    // Context
    segments.push(format!(
        "ctx:{:.1}k/{:.0}k ({:.0}%)",
        data.context_tokens_k, data.context_window_k, data.context_percent
    ));

    // Cumulative tokens
    segments.push(format!(
        "in:{} out:{}",
        format_tokens(data.input_tokens),
        format_tokens(data.output_tokens)
    ));

    // Effort (only if set)
    if let Some(effort) = data.effort {
        segments.push(format!("effort:{effort}"));
    }

    // Session
    if !data.session_id.is_empty() {
        segments.push(format!("session:{}", data.session_id));
    }

    // Plan mode badge
    if data.plan_mode {
        segments.push("PLAN".to_string());
    }

    format!(" {} ", segments.join(" | "))
}

/// Shorten a model identifier for display.
///
/// `claude-sonnet-4-5-20250929` → `sonnet-4.5`
/// `claude-opus-4-6` → `opus-4.6`
/// `claude-haiku-4-5-20251001` → `haiku-4.5`
pub fn shorten_model_name(model: &str) -> String {
    // Strip leading "claude-"
    let rest = model.strip_prefix("claude-").unwrap_or(model);

    // Strip trailing date stamp (-YYYYMMDD)
    let base = if rest.len() > 9 {
        let suffix = &rest[rest.len() - 9..];
        if suffix.starts_with('-')
            && suffix[1..].len() == 8
            && suffix[1..].chars().all(|c| c.is_ascii_digit())
        {
            &rest[..rest.len() - 9]
        } else {
            rest
        }
    } else {
        rest
    };

    // Convert "family-major-minor" to "family-major.minor"
    // e.g. "sonnet-4-5" → "sonnet-4.5", "opus-4-6" → "opus-4.6"
    // Only transform if the pattern is "word-digit-digit" (family-major-minor)
    let parts: Vec<&str> = base.rsplitn(3, '-').collect();
    if parts.len() == 3
        && parts[0].len() == 1
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && parts[1].len() == 1
        && parts[1].chars().all(|c| c.is_ascii_digit())
    {
        return format!("{}-{}.{}", parts[2], parts[1], parts[0]);
    }

    base.to_string()
}

/// Format a token count for compact display.
///
/// `0` → `0`, `999` → `999`, `1234` → `1.2k`, `1234567` → `1.2M`
pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Calculate the display width of a string (counts characters, not bytes).
fn display_width(s: &str) -> usize {
    s.chars().count()
}

/// Truncate a string to at most `max_width` display characters.
fn truncate_to_width(s: &str, max_width: usize) -> String {
    s.chars().take(max_width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // -- Byte capture writer for feeding to vt100 emulator --

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
            model: "test".to_string(),
            ..StatusLineData::default()
        }
    }

    /// Feed captured bytes into a real vt100 emulator and return cursor position (row, col).
    fn cursor_after(bytes: &[u8], rows: u16, cols: u16) -> (u16, u16) {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(bytes);
        parser.screen().cursor_position()
    }

    // -- Cursor preservation tests using vt100 terminal emulator --

    #[test]
    fn install_suspend_preserves_cursor() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter(buf.clone());

        let mut sl = StatusLine::new_with_writer(test_data(), 80, 24, Box::new(writer.clone()));

        // Simulate banner (use \r\n since vt100 needs CR for column reset)
        let _ = write!(writer.clone(), "chet v0.1 (model: test)\r\n");
        let _ = write!(writer.clone(), "Type your message.\r\n\r\n");

        let banner_pos = cursor_after(&buf.lock().unwrap(), 24, 80);
        assert_eq!(banner_pos.0, 3, "cursor should be at row 3 after banner");

        sl.set_cursor_row(3);
        sl.install();
        sl.suspend();

        let (row, _) = cursor_after(&buf.lock().unwrap(), 24, 80);
        assert_eq!(row, 3, "cursor row must be preserved after install+suspend");
    }

    #[test]
    fn full_repl_cycle_preserves_cursor() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter(buf.clone());

        let mut sl = StatusLine::new_with_writer(test_data(), 80, 24, Box::new(writer.clone()));

        let _ = write!(writer.clone(), "banner line 1\r\nbanner line 2\r\n\r\n");
        sl.set_cursor_row(3);
        sl.install();
        sl.suspend();

        let (row, _) = cursor_after(&buf.lock().unwrap(), 24, 80);
        assert_eq!(row, 3, "after first suspend");

        // Simulate: user typed, cursor at row 4
        let _ = write!(writer.clone(), "> hello\r\n");
        sl.set_cursor_row(4);
        sl.resume();

        let (row, _) = cursor_after(&buf.lock().unwrap(), 24, 80);
        assert_eq!(row, 4, "after resume");

        // Simulate: agent output
        let _ = write!(writer.clone(), "response line 1\r\nresponse line 2\r\n");
        let actual_row = cursor_after(&buf.lock().unwrap(), 24, 80).0;
        sl.set_cursor_row(actual_row);
        sl.suspend();

        let (row, _) = cursor_after(&buf.lock().unwrap(), 24, 80);
        assert_eq!(row, actual_row, "after second suspend");
    }

    #[test]
    fn teardown_preserves_cursor() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter(buf.clone());

        let mut sl = StatusLine::new_with_writer(test_data(), 80, 24, Box::new(writer.clone()));

        let _ = write!(writer.clone(), "\x1b[11;1H");
        sl.set_cursor_row(10);
        sl.install();
        sl.teardown();

        let (row, _) = cursor_after(&buf.lock().unwrap(), 24, 80);
        assert_eq!(row, 10, "after teardown");
    }

    #[test]
    fn resize_too_small_preserves_cursor() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter(buf.clone());

        let mut sl = StatusLine::new_with_writer(test_data(), 80, 24, Box::new(writer.clone()));

        let _ = write!(writer.clone(), "\x1b[9;1H");
        sl.set_cursor_row(8);
        sl.install();
        sl.resize(80, 2);

        let (row, _) = cursor_after(&buf.lock().unwrap(), 24, 80);
        assert_eq!(row, 8, "after resize too small");
    }

    #[test]
    fn banner_text_not_overwritten_by_prompt() {
        // Matches actual REPL flow: install+suspend BEFORE banner,
        // then banner prints at the cursor's natural position.
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter(buf.clone());

        let mut sl = StatusLine::new_with_writer(test_data(), 80, 24, Box::new(writer.clone()));

        // Install + suspend first (cursor starts at 0,0)
        sl.install();
        sl.suspend();

        // Banner prints at current cursor position (row 0 after DECSTBM homing)
        let _ = write!(writer.clone(), "chet v0.3.3 (model: test)\r\n");
        let _ = write!(writer.clone(), "Type your message.\r\n\r\n");

        // Prompt at current cursor position (row 3, after banner)
        let _ = write!(writer.clone(), "> ");

        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(&buf.lock().unwrap());

        let screen = parser.screen();
        let row0 = screen.contents_between(0, 0, 1, 80);
        let row3 = screen.contents_between(3, 0, 3, 80);

        assert!(
            row0.starts_with("chet v0.3.3"),
            "banner on row 0 must not be overwritten, got: {:?}",
            row0.trim()
        );
        assert!(
            row3.starts_with("> "),
            "prompt should be on row 3, got: {:?}",
            row3.trim()
        );
    }

    #[test]
    fn suspend_clears_stale_content_below_cursor() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter(buf.clone());

        let mut sl = StatusLine::new_with_writer(test_data(), 80, 24, Box::new(writer.clone()));

        // Put some "old content" on rows 5-7
        let _ = write!(writer.clone(), "\x1b[6;1Hold content line 1\r\n");
        let _ = write!(writer.clone(), "old content line 2\r\n");
        let _ = write!(writer.clone(), "old content line 3\r\n");

        // Install status line, set cursor to row 3 (above old content)
        sl.set_cursor_row(3);
        sl.install();
        sl.suspend();

        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(&buf.lock().unwrap());

        let screen = parser.screen();
        let row5 = screen.contents_between(5, 0, 6, 80);

        assert!(
            row5.trim().is_empty(),
            "stale content below cursor should be cleared by suspend, got: {:?}",
            row5.trim()
        );
    }

    // -- Existing tests --

    #[test]
    fn shorten_model_name_sonnet() {
        assert_eq!(
            shorten_model_name("claude-sonnet-4-5-20250929"),
            "sonnet-4.5"
        );
    }

    #[test]
    fn shorten_model_name_opus() {
        assert_eq!(shorten_model_name("claude-opus-4-6"), "opus-4.6");
    }

    #[test]
    fn shorten_model_name_haiku() {
        assert_eq!(shorten_model_name("claude-haiku-4-5-20251001"), "haiku-4.5");
    }

    #[test]
    fn shorten_model_name_unknown() {
        assert_eq!(shorten_model_name("gpt-4o"), "gpt-4o");
    }

    #[test]
    fn shorten_model_name_no_minor() {
        assert_eq!(shorten_model_name("claude-opus-4"), "opus-4");
    }

    #[test]
    fn format_tokens_zero() {
        assert_eq!(format_tokens(0), "0");
    }

    #[test]
    fn format_tokens_small() {
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(1234), "1.2k");
    }

    #[test]
    fn format_tokens_tens_of_thousands() {
        assert_eq!(format_tokens(42_100), "42.1k");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_234_567), "1.2M");
    }

    #[test]
    fn render_segments_basic() {
        let data = StatusLineData {
            model: "claude-sonnet-4-5-20250929".to_string(),
            session_id: "a1b2c3d4".to_string(),
            context_tokens_k: 42.1,
            context_window_k: 200.0,
            context_percent: 21.0,
            input_tokens: 12_300,
            output_tokens: 4_500,
            effort: Some(Effort::High),
            plan_mode: false,
            active_tool: None,
        };
        let rendered = render_segments(&data);
        assert!(rendered.contains("sonnet-4.5"));
        assert!(rendered.contains("ctx:42.1k/200k (21%)"));
        assert!(rendered.contains("in:12.3k out:4.5k"));
        assert!(rendered.contains("effort:high"));
        assert!(rendered.contains("session:a1b2c3d4"));
        assert!(!rendered.contains("PLAN"));
    }

    #[test]
    fn render_segments_plan_mode() {
        let data = StatusLineData {
            model: "claude-opus-4-6".to_string(),
            plan_mode: true,
            ..StatusLineData::default()
        };
        let rendered = render_segments(&data);
        assert!(rendered.contains("PLAN"));
    }

    #[test]
    fn render_segments_no_effort_when_none() {
        let data = StatusLineData {
            model: "claude-opus-4-6".to_string(),
            effort: None,
            ..StatusLineData::default()
        };
        let rendered = render_segments(&data);
        assert!(!rendered.contains("effort:"));
    }

    #[test]
    fn render_segments_active_tool_replaces_model() {
        let data = StatusLineData {
            model: "claude-sonnet-4-5-20250929".to_string(),
            active_tool: Some("Bash".to_string()),
            ..StatusLineData::default()
        };
        let rendered = render_segments(&data);
        assert!(rendered.contains("running: Bash"));
        assert!(!rendered.contains("sonnet"));
    }

    #[test]
    fn render_segments_mcp_tool_formatting() {
        let data = StatusLineData {
            model: "claude-sonnet-4-5-20250929".to_string(),
            active_tool: Some("mcp__github__search".to_string()),
            ..StatusLineData::default()
        };
        let rendered = render_segments(&data);
        assert!(rendered.contains("mcp: github>search"));
        assert!(!rendered.contains("running:"));
    }

    #[test]
    fn render_pads_to_width() {
        let sl = StatusLine {
            data: StatusLineData {
                model: "test".to_string(),
                ..StatusLineData::default()
            },
            terminal_width: 120,
            terminal_height: 24,
            installed: false,
            writer: Box::new(std::io::sink()),
            cursor_row: 0,
        };
        let rendered = sl.render();
        assert!(rendered.starts_with("\x1b[2;7m"));
        assert!(rendered.ends_with("\x1b[0m"));
        let inner = rendered
            .strip_prefix("\x1b[2;7m")
            .unwrap()
            .strip_suffix("\x1b[0m")
            .unwrap();
        assert_eq!(display_width(inner), 120);
    }

    #[test]
    fn render_truncates_narrow_terminal() {
        let sl = StatusLine {
            data: StatusLineData {
                model: "claude-sonnet-4-5-20250929".to_string(),
                session_id: "a1b2c3d4".to_string(),
                context_tokens_k: 42.1,
                context_window_k: 200.0,
                context_percent: 21.0,
                input_tokens: 12_300,
                output_tokens: 4_500,
                effort: Some(Effort::High),
                plan_mode: true,
                active_tool: None,
            },
            terminal_width: 30,
            terminal_height: 24,
            installed: false,
            writer: Box::new(std::io::sink()),
            cursor_row: 0,
        };
        let rendered = sl.render();
        let inner = rendered
            .strip_prefix("\x1b[2;7m")
            .unwrap()
            .strip_suffix("\x1b[0m")
            .unwrap();
        assert_eq!(display_width(inner), 30);
    }

    #[test]
    fn status_line_data_default() {
        let d = StatusLineData::default();
        assert!(d.model.is_empty());
        assert!(d.session_id.is_empty());
        assert_eq!(d.context_tokens_k, 0.0);
        assert_eq!(d.context_window_k, 0.0);
        assert_eq!(d.context_percent, 0.0);
        assert_eq!(d.input_tokens, 0);
        assert_eq!(d.output_tokens, 0);
        assert!(d.effort.is_none());
        assert!(!d.plan_mode);
        assert!(d.active_tool.is_none());
    }
}
