//! Session memory store skeleton.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::model::History;
use crate::runtime::error::RuntimeResult;

/// Session memory scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionMemoryScope {
    /// Session id.
    pub session_id: String,
    /// Optional actor id for isolation.
    pub actor_id: Option<String>,
}

/// One summarized memory turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMemoryTurn {
    /// Turn id.
    pub turn_id: String,
    /// User-side summary.
    pub user_summary: String,
    /// Answer-side summary.
    pub answer_summary: String,
    /// Intent id.
    pub intent: Option<String>,
    /// Metric id.
    pub metric: Option<String>,
    /// Asset id/name.
    pub asset: Option<String>,
    /// Time range label.
    pub time_range_label: Option<String>,
    /// Option id.
    pub option_id: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: u64,
}

/// Session memory value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMemory {
    /// Recent turns.
    pub recent_turns: Vec<SessionMemoryTurn>,
}

/// Session memory persistence contract.
#[async_trait]
pub trait SessionMemoryStore: Send + Sync {
    /// Get session memory.
    async fn get(&self, scope: &SessionMemoryScope) -> Option<SessionMemory>;

    /// Append one summarized turn.
    async fn append_turn(
        &self,
        scope: &SessionMemoryScope,
        turn: SessionMemoryTurn,
    ) -> SessionMemory;

    /// Clear session memory.
    async fn clear(&self, scope: &SessionMemoryScope);

    /// Load history for a session id.
    async fn load(&self, session_id: &str) -> RuntimeResult<Vec<History>>;

    /// Append one turn to a session id.
    async fn append(&self, session_id: &str, turn: History) -> RuntimeResult<()>;
}

/// In-memory session store used by the initial runtime pack.
#[derive(Debug, Default)]
pub struct InMemorySessionStore {
    max_turns: usize,
    sessions: Mutex<HashMap<String, SessionMemory>>,
    legacy_history: Mutex<HashMap<String, Vec<History>>>,
}

impl InMemorySessionStore {
    /// Create an in-memory store with a max recent-turn cap.
    pub fn new(max_turns: usize) -> Self {
        Self {
            max_turns,
            sessions: Mutex::new(HashMap::new()),
            legacy_history: Mutex::new(HashMap::new()),
        }
    }

    fn key(scope: &SessionMemoryScope) -> String {
        format!(
            "{}:{}",
            scope.actor_id.as_deref().unwrap_or("anonymous"),
            scope.session_id
        )
    }
}

#[async_trait]
impl SessionMemoryStore for InMemorySessionStore {
    async fn get(&self, scope: &SessionMemoryScope) -> Option<SessionMemory> {
        self.sessions.lock().await.get(&Self::key(scope)).cloned()
    }

    async fn append_turn(
        &self,
        scope: &SessionMemoryScope,
        turn: SessionMemoryTurn,
    ) -> SessionMemory {
        let mut sessions = self.sessions.lock().await;
        let memory = sessions.entry(Self::key(scope)).or_default();
        memory.recent_turns.push(turn);
        let overflow = memory.recent_turns.len().saturating_sub(self.max_turns);
        if overflow > 0 {
            memory.recent_turns.drain(0..overflow);
        }
        memory.clone()
    }

    async fn clear(&self, scope: &SessionMemoryScope) {
        self.sessions.lock().await.remove(&Self::key(scope));
    }

    async fn load(&self, session_id: &str) -> RuntimeResult<Vec<History>> {
        Ok(self
            .legacy_history
            .lock()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn append(&self, session_id: &str, turn: History) -> RuntimeResult<()> {
        self.legacy_history
            .lock()
            .await
            .entry(session_id.to_string())
            .or_default()
            .push(turn);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(user_prompt: &str) -> SessionMemoryTurn {
        SessionMemoryTurn {
            turn_id: user_prompt.to_string(),
            user_summary: user_prompt.to_string(),
            answer_summary: "answer".to_string(),
            intent: Some("revenue".to_string()),
            metric: Some("revenue".to_string()),
            asset: None,
            time_range_label: None,
            option_id: None,
            created_at_ms: 1,
        }
    }

    #[tokio::test]
    async fn append_caps_at_max_turns() {
        let store = InMemorySessionStore::new(2);
        let scope = SessionMemoryScope {
            session_id: "s1".into(),
            actor_id: Some("alice".into()),
        };

        store.append_turn(&scope, turn("one")).await;
        store.append_turn(&scope, turn("two")).await;
        store.append_turn(&scope, turn("three")).await;

        let memory = store.get(&scope).await.expect("memory should exist");
        assert_eq!(memory.recent_turns.len(), 2);
        assert_eq!(memory.recent_turns[0].user_summary, "two");
        assert_eq!(memory.recent_turns[1].user_summary, "three");
    }

    #[tokio::test]
    async fn clear_then_get_is_none() {
        let store = InMemorySessionStore::new(5);
        let scope = SessionMemoryScope {
            session_id: "s1".into(),
            actor_id: None,
        };

        store.append_turn(&scope, turn("one")).await;
        store.clear(&scope).await;

        assert!(store.get(&scope).await.is_none());
    }

    #[tokio::test]
    async fn key_isolates_by_actor() {
        let store = InMemorySessionStore::new(5);
        let alice = SessionMemoryScope {
            session_id: "same-session".into(),
            actor_id: Some("alice".into()),
        };
        let bob = SessionMemoryScope {
            session_id: "same-session".into(),
            actor_id: Some("bob".into()),
        };

        store.append_turn(&alice, turn("alice-only")).await;

        assert!(store.get(&bob).await.is_none());
    }
}
