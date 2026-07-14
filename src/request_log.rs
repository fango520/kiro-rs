use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogEntry {
    pub id: String,
    pub timestamp: String,
    pub api_key_id: String,
    pub api_key_name: String,
    pub api_key_prefix: String,
    pub model: String,
    pub stream: bool,
    pub status: u16,
    pub success: bool,
    pub credential_id: Option<u64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub duration_ms: u128,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogSummary {
    pub request_count: usize,
    pub success_count: usize,
    pub error_count: usize,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogListResponse {
    pub logs: Vec<RequestLogEntry>,
    pub summary: RequestLogSummary,
}

#[derive(Clone)]
pub struct RequestLogStore {
    path: PathBuf,
    max_entries: usize,
    entries: Arc<Mutex<VecDeque<RequestLogEntry>>>,
}

impl RequestLogStore {
    pub fn new(config_dir: impl AsRef<Path>, max_entries: usize) -> Self {
        let path = config_dir.as_ref().join("request_logs.jsonl");
        let mut entries = VecDeque::new();
        if let Ok(raw) = std::fs::read_to_string(&path) {
            for line in raw.lines().rev().take(max_entries).collect::<Vec<_>>().into_iter().rev() {
                if let Ok(entry) = serde_json::from_str::<RequestLogEntry>(line) { entries.push_back(entry); }
            }
        }
        Self { path, max_entries, entries: Arc::new(Mutex::new(entries)) }
    }

    pub fn record(&self, mut entry: RequestLogEntry) {
        if entry.id.is_empty() { entry.id = Uuid::new_v4().to_string(); }
        if entry.timestamp.is_empty() { entry.timestamp = Utc::now().to_rfc3339(); }
        entry.total_tokens = entry.input_tokens.saturating_add(entry.output_tokens);
        {
            let mut entries = self.entries.lock();
            entries.push_back(entry.clone());
            while entries.len() > self.max_entries { entries.pop_front(); }
        }
        if let Some(parent) = self.path.parent() { let _ = std::fs::create_dir_all(parent); }
        match std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            Ok(mut f) => {
                if let Ok(raw) = serde_json::to_string(&entry) { let _ = writeln!(f, "{}", raw); }
            }
            Err(err) => tracing::warn!("写入请求日志失败: {}", err),
        }
    }

    pub fn list(&self, limit: usize) -> RequestLogListResponse {
        let limit = limit.clamp(1, self.max_entries);
        let entries = self.entries.lock();
        let logs: Vec<_> = entries.iter().rev().take(limit).cloned().collect();
        let summary = summarize(entries.iter());
        RequestLogListResponse { logs, summary }
    }

    pub fn summary(&self) -> RequestLogSummary {
        let entries = self.entries.lock();
        summarize(entries.iter())
    }
}

fn summarize<'a>(entries: impl Iterator<Item=&'a RequestLogEntry>) -> RequestLogSummary {
    let mut summary = RequestLogSummary { request_count: 0, success_count: 0, error_count: 0, input_tokens: 0, output_tokens: 0, total_tokens: 0 };
    for e in entries {
        summary.request_count += 1;
        if e.success { summary.success_count += 1; } else { summary.error_count += 1; }
        summary.input_tokens = summary.input_tokens.saturating_add(e.input_tokens.max(0));
        summary.output_tokens = summary.output_tokens.saturating_add(e.output_tokens.max(0));
    }
    summary.total_tokens = summary.input_tokens.saturating_add(summary.output_tokens);
    summary
}
