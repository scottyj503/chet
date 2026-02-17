//! Persistent session storage backed by JSON files.

use crate::error::SessionError;
use crate::types::{Session, SessionSummary};
use std::path::PathBuf;
use uuid::Uuid;

/// File-based session store. Each session is a JSON file in `sessions_dir`.
pub struct SessionStore {
    sessions_dir: PathBuf,
}

impl SessionStore {
    /// Create a new store, ensuring the sessions directory exists.
    pub async fn new(config_dir: PathBuf) -> Result<Self, SessionError> {
        let sessions_dir = config_dir.join("sessions");
        tokio::fs::create_dir_all(&sessions_dir).await?;
        Ok(Self { sessions_dir })
    }

    /// Save a session to disk (atomic write: .tmp → rename).
    pub async fn save(&self, session: &Session) -> Result<(), SessionError> {
        let path = self.session_path(session.id);
        let tmp_path = path.with_extension("tmp");
        let json = serde_json::to_string_pretty(session)?;
        tokio::fs::write(&tmp_path, json).await?;
        tokio::fs::rename(&tmp_path, &path).await?;
        Ok(())
    }

    /// Load a session by exact UUID.
    pub async fn load(&self, id: Uuid) -> Result<Session, SessionError> {
        let path = self.session_path(id);
        if !path.exists() {
            return Err(SessionError::NotFound { id });
        }
        let data = tokio::fs::read_to_string(&path).await?;
        let session: Session = serde_json::from_str(&data)?;
        Ok(session)
    }

    /// Load a session by ID prefix. Errors if ambiguous (multiple matches).
    pub async fn load_by_prefix(&self, prefix: &str) -> Result<Session, SessionError> {
        let prefix_lower = prefix.to_lowercase();
        let mut matches = Vec::new();

        let mut entries = tokio::fs::read_dir(&self.sessions_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".json") {
                let stem = name_str.trim_end_matches(".json");
                if stem.to_lowercase().starts_with(&prefix_lower) {
                    if let Ok(id) = Uuid::parse_str(stem) {
                        matches.push(id);
                    }
                }
            }
        }

        match matches.len() {
            0 => Err(SessionError::PrefixNotFound {
                prefix: prefix.to_string(),
            }),
            1 => self.load(matches[0]).await,
            count => Err(SessionError::AmbiguousPrefix {
                prefix: prefix.to_string(),
                count,
            }),
        }
    }

    /// List all sessions, sorted by updated_at descending (most recent first).
    pub async fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
        let mut summaries = Vec::new();

        let mut entries = tokio::fs::read_dir(&self.sessions_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".json") {
                let path = entry.path();
                match tokio::fs::read_to_string(&path).await {
                    Ok(data) => match serde_json::from_str::<Session>(&data) {
                        Ok(session) => summaries.push(session.to_summary()),
                        Err(e) => {
                            tracing::warn!("Failed to parse session {}: {}", name_str, e);
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Failed to read session {}: {}", name_str, e);
                    }
                }
            }
        }

        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(summaries)
    }

    /// Delete a session file.
    pub async fn delete(&self, id: Uuid) -> Result<(), SessionError> {
        let path = self.session_path(id);
        if !path.exists() {
            return Err(SessionError::NotFound { id });
        }
        tokio::fs::remove_file(&path).await?;
        Ok(())
    }

    /// Write a compaction archive as a markdown file alongside the session.
    pub async fn write_compaction_archive(
        &self,
        session_id: Uuid,
        compaction_number: u32,
        markdown: &str,
    ) -> Result<PathBuf, SessionError> {
        let filename = format!("{}-compact-{}.md", session_id, compaction_number);
        let path = self.sessions_dir.join(filename);
        tokio::fs::write(&path, markdown).await?;
        Ok(path)
    }

    fn session_path(&self, id: Uuid) -> PathBuf {
        self.sessions_dir.join(format!("{id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Session;
    use chet_types::{ContentBlock, Message, Role};
    use tempfile::TempDir;

    async fn test_store() -> (SessionStore, TempDir) {
        let tmp = TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path().to_path_buf()).await.unwrap();
        (store, tmp)
    }

    fn test_session() -> Session {
        let mut session = Session::new("claude-test".into(), "/tmp".into());
        session.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".into(),
            }],
        });
        session
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let (store, _tmp) = test_store().await;
        let session = test_session();
        let id = session.id;

        store.save(&session).await.unwrap();
        let loaded = store.load(id).await.unwrap();

        assert_eq!(loaded.id, id);
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.metadata.model, "claude-test");
    }

    #[tokio::test]
    async fn load_nonexistent_returns_not_found() {
        let (store, _tmp) = test_store().await;
        let result = store.load(Uuid::new_v4()).await;
        assert!(matches!(result, Err(SessionError::NotFound { .. })));
    }

    #[tokio::test]
    async fn load_by_prefix_exact() {
        let (store, _tmp) = test_store().await;
        let session = test_session();
        let id_str = session.id.to_string();
        store.save(&session).await.unwrap();

        let loaded = store.load_by_prefix(&id_str).await.unwrap();
        assert_eq!(loaded.id, session.id);
    }

    #[tokio::test]
    async fn load_by_prefix_short() {
        let (store, _tmp) = test_store().await;
        let session = test_session();
        let prefix = session.id.to_string()[..8].to_string();
        store.save(&session).await.unwrap();

        let loaded = store.load_by_prefix(&prefix).await.unwrap();
        assert_eq!(loaded.id, session.id);
    }

    #[tokio::test]
    async fn load_by_prefix_not_found() {
        let (store, _tmp) = test_store().await;
        let result = store.load_by_prefix("ffffffff").await;
        assert!(matches!(result, Err(SessionError::PrefixNotFound { .. })));
    }

    #[tokio::test]
    async fn list_empty() {
        let (store, _tmp) = test_store().await;
        let summaries = store.list().await.unwrap();
        assert!(summaries.is_empty());
    }

    #[tokio::test]
    async fn list_multiple() {
        let (store, _tmp) = test_store().await;
        let s1 = test_session();
        let s2 = test_session();
        store.save(&s1).await.unwrap();
        store.save(&s2).await.unwrap();

        let summaries = store.list().await.unwrap();
        assert_eq!(summaries.len(), 2);
    }

    #[tokio::test]
    async fn write_compaction_archive() {
        let (store, _tmp) = test_store().await;
        let session = test_session();

        let path = store
            .write_compaction_archive(session.id, 1, "# Archive\nHello world")
            .await
            .unwrap();

        assert!(path.exists());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("# Archive"));
    }
}
