//! System prompt construction for normal and plan modes.

use chet_types::{ContentBlock, Message, Role};

/// Build a user message from text input.
pub(crate) fn user_message(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    }
}

/// Append persistent-memory instructions and content to a system prompt.
pub(crate) fn append_memory_instructions(prompt: &mut String, memory: &str) {
    prompt.push_str(
        "\n\n# Persistent Memory\n\n\
        You have access to persistent memory that survives across sessions via the \
        MemoryRead and MemoryWrite tools.\n\
        - Use MemoryRead to check existing memory before writing.\n\
        - When writing, provide the complete updated content for the scope (global or project).\n\
        - Use 'global' scope for cross-project preferences, 'project' for project-specific notes.\n\
        - Save things worth remembering: user preferences, project conventions, key decisions.\n\
        - When the user explicitly asks you to remember something, save it immediately.",
    );
    if !memory.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(memory);
    }
}

/// Build the default system prompt.
pub(crate) fn system_prompt(cwd: &std::path::Path, memory: &str) -> String {
    let mut prompt = format!(
        "You are Chet, an AI coding assistant running in a terminal. \
         You help users with software engineering tasks by reading, writing, \
         and editing code files, running commands, and searching codebases.\n\n\
         Current working directory: {}\n\n\
         Use the available tools to assist the user. Be concise and helpful.",
        cwd.display()
    );
    append_memory_instructions(&mut prompt, memory);
    prompt
}

/// Build the plan-mode system prompt.
pub(crate) fn plan_system_prompt(cwd: &std::path::Path, memory: &str) -> String {
    let mut prompt = format!(
        "You are Chet, an AI coding assistant running in PLAN MODE.\n\n\
         Current working directory: {}\n\n\
         In plan mode, you can ONLY use read-only tools (Read, Glob, Grep) to explore the codebase.\n\
         You CANNOT modify files, run commands, or make any changes.\n\n\
         Your task is to:\n\
         1. Explore the codebase using the available read-only tools\n\
         2. Understand the existing code structure and patterns\n\
         3. Produce a clear, structured implementation plan in markdown\n\n\
         Your plan should include:\n\
         - Summary of proposed changes\n\
         - Files to modify or create\n\
         - Step-by-step implementation approach\n\
         - Key considerations or risks\n\n\
         Be thorough in exploration but concise in your plan.",
        cwd.display()
    );
    append_memory_instructions(&mut prompt, memory);
    prompt
}

/// Print token usage to stderr.
pub(crate) fn print_usage(usage: &chet_types::Usage) {
    eprintln!(
        "Tokens — input: {}, output: {}, cache read: {}, cache write: {}",
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation_input_tokens
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_system_prompt_contains_key_directives() {
        let prompt = plan_system_prompt(std::path::Path::new("/tmp"), "");
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("read-only"));
        assert!(prompt.contains("/tmp"));
        assert!(prompt.contains("Read"));
        assert!(prompt.contains("Glob"));
        assert!(prompt.contains("Grep"));
    }

    #[test]
    fn system_prompt_includes_memory() {
        let prompt = system_prompt(std::path::Path::new("/tmp"), "# Memory\n\nTest memory");
        assert!(prompt.contains("Test memory"));
        assert!(prompt.contains("MemoryRead"));
        assert!(prompt.contains("MemoryWrite"));
    }

    #[test]
    fn system_prompt_empty_memory() {
        let prompt = system_prompt(std::path::Path::new("/tmp"), "");
        assert!(prompt.contains("MemoryRead"));
        assert!(prompt.contains("MemoryWrite"));
        assert!(!prompt.contains("# Memory"));
    }
}
