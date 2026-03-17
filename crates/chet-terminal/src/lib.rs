//! Terminal UI, streaming markdown renderer, and input handling for Chet.

mod buffer;
mod completion;
mod editor;
pub mod highlight;
mod history;
mod inline;
mod keys;
pub mod markdown;
mod render;
pub mod spinner;
pub mod statusline;
pub mod style;
mod table;

pub use completion::{Completer, SlashCommandCompleter};
pub use editor::{LineEditor, ReadLineResult};
pub use markdown::StreamingMarkdownRenderer;
pub use statusline::{StatusLine, StatusLineData};
