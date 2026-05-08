//! Conversation history. One JSON record per conversation in
//! `conversations.json` (tauri-plugin-store), keyed by conversation id.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rig::message::Message;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;
use uuid::Uuid;

pub const STORE_FILE: &str = "conversations.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub messages: Vec<Message>,
    pub updated_at: u64,
}

impl Conversation {
    pub fn new(first_msg: &str) -> Self {
        let title: String = first_msg.chars().take(60).collect();
        Self {
            id: Uuid::new_v4().to_string(),
            title: if title.trim().is_empty() {
                "New chat".into()
            } else {
                title
            },
            messages: Vec::new(),
            updated_at: now_secs(),
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = now_secs();
    }
}

/// Stripped-down list-view shape — drops `messages` to keep the list payload small.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    pub message_count: usize,
}

impl From<&Conversation> for ConversationSummary {
    fn from(c: &Conversation) -> Self {
        Self {
            id: c.id.clone(),
            title: c.title.clone(),
            updated_at: c.updated_at,
            message_count: c.messages.len(),
        }
    }
}

pub fn save<R: Runtime>(app: &AppHandle<R>, conv: &Conversation) -> Result<()> {
    let store = app.store(STORE_FILE).context("opening conversations store")?;
    store.set(
        &conv.id,
        serde_json::to_value(conv).context("serialising conversation")?,
    );
    store.save().context("flushing conversations store")?;
    Ok(())
}

pub fn load<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<Option<Conversation>> {
    let store = app.store(STORE_FILE).context("opening conversations store")?;
    Ok(store
        .get(id)
        .and_then(|v| serde_json::from_value(v).ok()))
}

pub fn list<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<ConversationSummary>> {
    let store = app.store(STORE_FILE).context("opening conversations store")?;
    let mut summaries: Vec<ConversationSummary> = store
        .entries()
        .into_iter()
        .filter_map(|(_, v)| serde_json::from_value::<Conversation>(v).ok())
        .map(|c| ConversationSummary::from(&c))
        .collect();
    summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(summaries)
}

pub fn delete<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<()> {
    let store = app.store(STORE_FILE).context("opening conversations store")?;
    store.delete(id);
    store.save().context("flushing conversations store")?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
