//! Terminal UI, streaming markdown renderer, and input handling for Chet.

mod buffer;
mod completion;
mod editor;
pub mod highlight;
mod history;
mod keys;
pub mod markdown;
mod render;
pub mod spinner;
pub mod style;

pub use completion::{Completer, SlashCommandCompleter};
pub use editor::{LineEditor, ReadLineResult};
pub use markdown::StreamingMarkdownRenderer;
