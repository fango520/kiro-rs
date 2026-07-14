use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::common::auth;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyRecord {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing)]
    pub key: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub enabled: bool,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub request_count: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyView {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
    pub enabled: bool,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub request_count: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedApiKey {
    pub id: String,
    pub name: String,
    pub key: String,
    pub key_prefix: String,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ApiKeyContext {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiKeyRequest {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetApiKeyEnabledRequest {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiKeyFileRecord {
    id: String,
    name: String,
    key_hash: String,
    key_prefix: String,
    enabled: bool,
    created_at: String,
    last_used_at: Option<String>,
    request_count: u64,
    input_tokens: i64,
    output_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ApiKeyFile {
    keys: Vec<ApiKeyFileRecord>,
}

#[derive(Clone)]
pub struct ApiKeyStore {
    path: PathBuf,
    keys: Arc<RwLock<Vec<ApiKeyRecord>>>,
}

impl ApiKeyStore {
    pub fn new(config_dir: impl AsRef<Path>, default_api_key: String) -> Self {
        let path = config_dir.as_ref().join("api_keys.json");
        let now = Utc::now().to_rfc3339();
        let mut records = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<ApiKeyFile>(&raw).ok())
                .map(|file| {
                    file.keys
                        .into_iter()
                        .map(|item| ApiKeyRecord {
                            id: item.id,
                            name: item.name,
                            key: String::new(),
                            key_hash: item.key_hash,
                            key_prefix: item.key_prefix,
                            enabled: item.enabled,
                            created_at: item.created_at,
                            last_used_at: item.last_used_at,
                            request_count: item.request_count,
                            input_tokens: item.input_tokens,
                            output_tokens: item.output_tokens,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let default_hash = hash_key(&default_api_key);
        if !records.iter().any(|r| r.key_hash == default_hash) {
            records.insert(0, ApiKeyRecord {
                id: "default".to_string(),
                name: "Default API Key".to_string(),
                key: default_api_key.clone(),
                key_hash: default_hash,
                key_prefix: mask_key_prefix(records.first().map(|_| "").unwrap_or("")),
                enabled: true,
                created_at: now,
                last_used_at: None,
                request_count: 0,
                input_tokens: 0,
                output_tokens: 0,
            });
        }
        if let Some(default) = records.iter_mut().find(|r| r.id == "default") {
            default.key = default_api_key.clone();
            default.key_hash = hash_key(&default_api_key);
            default.key_prefix = mask_key_prefix(&default_api_key);
        }

        let store = Self { path, keys: Arc::new(RwLock::new(records)) };
        store.save();
        store
    }

    pub fn validate(&self, candidate: &str) -> Option<ApiKeyContext> {
        let hash = hash_key(candidate);
        let mut keys = self.keys.write();
        let found = keys.iter_mut().find(|record| {
            record.enabled && (record.key_hash == hash || (!record.key.is_empty() && auth::constant_time_eq(candidate, &record.key)))
        })?;
        found.last_used_at = Some(Utc::now().to_rfc3339());
        let ctx = ApiKeyContext { id: found.id.clone(), name: found.name.clone(), key_prefix: found.key_prefix.clone() };
        drop(keys);
        self.save();
        Some(ctx)
    }

    pub fn list(&self) -> Vec<ApiKeyView> {
        let mut items: Vec<ApiKeyView> = self.keys.read().iter().map(|r| ApiKeyView {
            id: r.id.clone(), name: r.name.clone(), key_prefix: r.key_prefix.clone(), enabled: r.enabled,
            created_at: r.created_at.clone(), last_used_at: r.last_used_at.clone(), request_count: r.request_count,
            input_tokens: r.input_tokens, output_tokens: r.output_tokens,
        }).collect();
        items.sort_by(|a,b| a.created_at.cmp(&b.created_at));
        items
    }

    pub fn create(&self, name: String) -> CreatedApiKey {
        let key = format!("sk-{}", Uuid::new_v4().to_string().replace('-', ""));
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();
        let record = ApiKeyRecord {
            id: id.clone(), name: name.trim().to_string(), key: key.clone(), key_hash: hash_key(&key), key_prefix: mask_key_prefix(&key),
            enabled: true, created_at: created_at.clone(), last_used_at: None, request_count: 0, input_tokens: 0, output_tokens: 0,
        };
        self.keys.write().push(record);
        self.save();
        CreatedApiKey { id, name, key: key.clone(), key_prefix: mask_key_prefix(&key), enabled: true, created_at }
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> anyhow::Result<()> {
        let mut keys = self.keys.write();
        let item = keys.iter_mut().find(|r| r.id == id).ok_or_else(|| anyhow::anyhow!("API Key not found"))?;
        item.enabled = enabled;
        drop(keys);
        self.save();
        Ok(())
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        if id == "default" { anyhow::bail!("默认 API Key 不能删除，可以禁用"); }
        let mut keys = self.keys.write();
        let before = keys.len();
        keys.retain(|r| r.id != id);
        if keys.len() == before { anyhow::bail!("API Key not found"); }
        drop(keys);
        self.save();
        Ok(())
    }

    pub fn record_usage(&self, id: &str, input_tokens: i64, output_tokens: i64) {
        let mut keys = self.keys.write();
        if let Some(item) = keys.iter_mut().find(|r| r.id == id) {
            item.request_count = item.request_count.saturating_add(1);
            item.input_tokens = item.input_tokens.saturating_add(input_tokens.max(0));
            item.output_tokens = item.output_tokens.saturating_add(output_tokens.max(0));
            item.last_used_at = Some(Utc::now().to_rfc3339());
        }
        drop(keys);
        self.save();
    }

    fn save(&self) {
        let file = ApiKeyFile { keys: self.keys.read().iter().map(|r| ApiKeyFileRecord {
            id: r.id.clone(), name: r.name.clone(), key_hash: r.key_hash.clone(), key_prefix: r.key_prefix.clone(), enabled: r.enabled,
            created_at: r.created_at.clone(), last_used_at: r.last_used_at.clone(), request_count: r.request_count,
            input_tokens: r.input_tokens, output_tokens: r.output_tokens,
        }).collect() };
        if let Some(parent) = self.path.parent() { let _ = std::fs::create_dir_all(parent); }
        match serde_json::to_string_pretty(&file) {
            Ok(raw) => { if let Err(err) = std::fs::write(&self.path, raw) { tracing::warn!("保存 API Key 文件失败: {}", err); } }
            Err(err) => tracing::warn!("序列化 API Key 文件失败: {}", err),
        }
    }
}

fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

fn mask_key_prefix(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.len() <= 12 { return trimmed.to_string(); }
    format!("{}...{}", &trimmed[..8], &trimmed[trimmed.len()-4..])
}
