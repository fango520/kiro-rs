//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求
//! 支持多凭据故障转移和重试
//! 支持按凭据级 endpoint 切换不同 Kiro API 端点

use reqwest::Client;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::endpoint::{KiroEndpoint, RequestContext};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::MultiTokenManager;
use crate::model::config::TlsBackend;
use parking_lot::Mutex;

/// API 调用结果
pub struct ApiCallResult {
    pub response: reqwest::Response,
    pub credential_id: u64,
}

/// 每个凭据的最大重试次数
const MAX_RETRIES_PER_CREDENTIAL: usize = 3;

/// 总重试次数硬上限（避免无限重试）
const MAX_TOTAL_RETRIES: usize = 9;

/// 动态模型列表缓存有效期
const MODEL_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

const KIRO_DEFAULT_MODEL_ID: &str = "claude-sonnet-4.5";

/// Kiro 官方模型信息
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroAvailableModel {
    pub model_id: String,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub model_provider: Option<String>,
    #[serde(default)]
    pub token_limits: Option<KiroModelTokenLimits>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroModelTokenLimits {
    pub max_input_tokens: Option<i32>,
    pub max_output_tokens: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAvailableModelsResponse {
    #[serde(default)]
    models: Vec<KiroAvailableModel>,
    next_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroProfile {
    arn: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAvailableProfilesResponse {
    #[serde(default)]
    profiles: Vec<KiroProfile>,
}

#[derive(Clone)]
struct ModelCache {
    models: Vec<KiroAvailableModel>,
    fetched_at: Instant,
}

/// Kiro API Provider
///
/// 核心组件，负责与 Kiro API 通信
/// 支持多凭据故障转移和重试机制
/// 按凭据 `endpoint` 字段选择 [`KiroEndpoint`] 实现
pub struct KiroProvider {
    token_manager: Arc<MultiTokenManager>,
    /// 全局代理配置（用于凭据无自定义代理时的回退）
    global_proxy: Option<ProxyConfig>,
    /// Client 缓存：key = effective proxy config, value = reqwest::Client
    /// 不同代理配置的凭据使用不同的 Client，共享相同代理的凭据复用 Client
    client_cache: Mutex<HashMap<Option<ProxyConfig>, Client>>,
    /// TLS 后端配置
    tls_backend: TlsBackend,
    /// 端点实现注册表（key: endpoint 名称）
    endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
    /// 默认端点名称（凭据未指定 endpoint 时使用）
    default_endpoint: String,
    /// ListAvailableModels 缓存
    model_cache: Mutex<Option<ModelCache>>,
}

impl KiroProvider {
    /// 创建带代理配置和端点注册表的 KiroProvider 实例
    ///
    /// # Arguments
    /// * `token_manager` - 多凭据 Token 管理器
    /// * `proxy` - 全局代理配置
    /// * `endpoints` - 端点名 → 实现的注册表（至少包含 `default_endpoint` 对应条目）
    /// * `default_endpoint` - 凭据未显式指定 endpoint 时使用的名称
    pub fn with_proxy(
        token_manager: Arc<MultiTokenManager>,
        proxy: Option<ProxyConfig>,
        endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
        default_endpoint: String,
    ) -> Self {
        assert!(
            endpoints.contains_key(&default_endpoint),
            "默认端点 {} 未在 endpoints 注册表中",
            default_endpoint
        );
        let tls_backend = token_manager.config().tls_backend;
        // 预热：构建全局代理对应的 Client
        let initial_client =
            build_client(proxy.as_ref(), 720, tls_backend).expect("创建 HTTP 客户端失败");
        let mut cache = HashMap::new();
        cache.insert(proxy.clone(), initial_client);

        Self {
            token_manager,
            global_proxy: proxy,
            client_cache: Mutex::new(cache),
            tls_backend,
            endpoints,
            default_endpoint,
            model_cache: Mutex::new(None),
        }
    }

    /// 根据凭据的代理配置获取（或创建并缓存）对应的 reqwest::Client
    fn client_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Client> {
        let effective = credentials.effective_proxy(self.global_proxy.as_ref());
        let mut cache = self.client_cache.lock();
        if let Some(client) = cache.get(&effective) {
            return Ok(client.clone());
        }
        let client = build_client(effective.as_ref(), 720, self.tls_backend)?;
        cache.insert(effective, client.clone());
        Ok(client)
    }

    /// 根据凭据选择 endpoint 实现
    fn endpoint_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Arc<dyn KiroEndpoint>> {
        let name = credentials
            .endpoint
            .as_deref()
            .unwrap_or(&self.default_endpoint);
        self.endpoints
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("未知端点: {}", name))
    }

    /// 获取 Kiro 官方可用模型列表（带 5 分钟缓存）。
    pub async fn list_available_models(&self) -> anyhow::Result<Vec<KiroAvailableModel>> {
        {
            let cache = self.model_cache.lock();
            if let Some(cache) = cache.as_ref() {
                if cache.fetched_at.elapsed() <= MODEL_CACHE_TTL {
                    return Ok(cache.models.clone());
                }
            }
        }

        let models = self.fetch_available_models().await?;
        *self.model_cache.lock() = Some(ModelCache {
            models: models.clone(),
            fetched_at: Instant::now(),
        });
        Ok(models)
    }

    async fn fetch_available_models(&self) -> anyhow::Result<Vec<KiroAvailableModel>> {
        let ctx = self.token_manager.acquire_context(None).await?;
        let config = self.token_manager.config();
        let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);
        let endpoint = self.endpoint_for(&ctx.credentials)?;
        let credentials = self
            .resolve_and_persist_profile_arn(
                ctx.id,
                &ctx.credentials,
                &ctx.token,
                &machine_id,
                endpoint.as_ref(),
            )
            .await;

        self.fetch_available_models_for(&credentials, &ctx.token, &machine_id, endpoint.as_ref())
            .await
    }

    async fn fetch_available_models_for(
        &self,
        credentials: &KiroCredentials,
        token: &str,
        machine_id: &str,
        endpoint: &dyn KiroEndpoint,
    ) -> anyhow::Result<Vec<KiroAvailableModel>> {
        let config = self.token_manager.config();
        let rctx = RequestContext {
            credentials,
            token,
            machine_id,
            config,
        };

        let url = endpoint
            .models_url(&rctx)
            .ok_or_else(|| anyhow::anyhow!("端点不支持动态模型列表: {}", endpoint.name()))?;

        let mut all_models = Vec::new();
        let mut next_token: Option<String> = None;

        loop {
            let mut params = vec![
                ("origin", "AI_EDITOR".to_string()),
                ("maxResults", "50".to_string()),
            ];

            if let Some(profile_arn) = credentials.resolved_profile_arn() {
                params.push(("profileArn", profile_arn));
            }
            if let Some(token) = next_token.as_ref() {
                params.push(("nextToken", token.clone()));
            }

            let base = self
                .client_for(credentials)?
                .get(&url)
                .query(&params)
                .header("Accept", "application/json")
                .header("Connection", "close");
            let request = endpoint.decorate_models(base, &rctx);
            let response = request.send().await?;
            let status = response.status();

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("ListAvailableModels 请求失败: {} {}", status, body);
            }

            let page: ListAvailableModelsResponse = response.json().await?;
            all_models.extend(page.models);

            next_token = page.next_token.filter(|token| !token.trim().is_empty());
            if next_token.is_none() {
                break;
            }
        }

        Ok(all_models)
    }

    async fn resolve_and_persist_profile_arn(
        &self,
        credential_id: u64,
        credentials: &KiroCredentials,
        token: &str,
        machine_id: &str,
        endpoint: &dyn KiroEndpoint,
    ) -> KiroCredentials {
        let mut resolved = credentials.clone();
        if !resolved.should_fetch_profile_arn() {
            return resolved;
        }

        match self
            .fetch_enterprise_profile_arn(&resolved, token, machine_id, endpoint)
            .await
        {
            Ok(Some(profile_arn)) => {
                tracing::info!(
                    "凭据 #{} 已通过 ListAvailableProfiles 获取 profileArn",
                    credential_id
                );
                resolved.profile_arn = Some(profile_arn.clone());
                if let Err(err) = self
                    .token_manager
                    .update_profile_arn(credential_id, profile_arn)
                {
                    tracing::warn!(
                        "凭据 #{} profileArn 自愈持久化失败（不影响本次请求）: {}",
                        credential_id,
                        err
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::debug!(
                    "凭据 #{} ListAvailableProfiles 未获取到 profileArn: {}",
                    credential_id,
                    err
                );
            }
        }

        resolved
    }

    async fn fetch_enterprise_profile_arn(
        &self,
        credentials: &KiroCredentials,
        token: &str,
        machine_id: &str,
        endpoint: &dyn KiroEndpoint,
    ) -> anyhow::Result<Option<String>> {
        let config = self.token_manager.config();
        let rctx = RequestContext {
            credentials,
            token,
            machine_id,
            config,
        };

        let Some(url) = endpoint.profiles_url(&rctx) else {
            return Ok(None);
        };

        let base = self
            .client_for(credentials)?
            .post(&url)
            .body("{}")
            .header("content-type", "application/json")
            .header("Connection", "close");
        let request = endpoint.decorate_profiles(base, &rctx);
        let response = request.send().await?;
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            tracing::debug!(
                "ListAvailableProfiles 请求失败: {} {}",
                status,
                body.chars().take(200).collect::<String>()
            );
            return Ok(None);
        }

        let data: ListAvailableProfilesResponse = response.json().await?;
        Ok(data
            .profiles
            .into_iter()
            .filter_map(|profile| profile.arn)
            .map(|arn| arn.trim().to_string())
            .find(|arn| !arn.is_empty()))
    }

    /// 发送非流式 API 请求
    ///
    /// 支持多凭据故障转移（见 [`Self::call_api_with_retry`]）
    pub async fn call_api(&self, request_body: &str) -> anyhow::Result<ApiCallResult> {
        self.call_api_with_retry(request_body, false).await
    }

    /// 发送流式 API 请求
    pub async fn call_api_stream(&self, request_body: &str) -> anyhow::Result<ApiCallResult> {
        self.call_api_with_retry(request_body, true).await
    }

    /// 发送 MCP API 请求（WebSearch 等工具调用）
    pub async fn call_mcp(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        self.call_mcp_with_retry(request_body).await
    }

    /// 内部方法：带重试逻辑的 MCP API 调用
    async fn call_mcp_with_retry(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let total_credentials = self.token_manager.total_count();
        let max_retries = (total_credentials * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();

        for attempt in 0..max_retries {
            // MCP 调用（WebSearch 等工具）不涉及模型选择，无需按模型过滤凭据
            let ctx = match self.token_manager.acquire_context(None).await {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    last_error = Some(e);
                    // endpoint 解析失败：记为失败，换下一张凭据
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.mcp_url(&rctx);
            let body = endpoint.transform_mcp_body(request_body, &rctx);

            let base = self
                .client_for(&ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json")
                .header("Connection", "close");
            let request = endpoint.decorate_mcp(base, &rctx);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "MCP 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();

            // 成功响应
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                return Ok(response);
            }

            // 失败响应
            let body = response.text().await.unwrap_or_default();

            // 402 额度用尽
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 400 Bad Request
            if status.as_u16() == 400 {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 401/403 凭据问题
            if matches!(status.as_u16(), 401 | 403) {
                if endpoint.is_account_suspended(&body) {
                    tracing::error!(
                        "凭据 #{} 已被 Kiro 官方暂停/封禁: {} {}",
                        ctx.id,
                        status,
                        body
                    );
                    let has_available = self.token_manager.report_account_suspended(ctx.id);
                    if !has_available {
                        anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                    }
                    last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                    continue;
                }

                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self
                        .token_manager
                        .force_refresh_token_for(ctx.id)
                        .await
                        .is_ok()
                    {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 瞬态错误
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                tracing::warn!(
                    "MCP 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx
            if status.is_client_error() {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 兜底
            last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
            if attempt + 1 < max_retries {
                sleep(Self::retry_delay(attempt)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("MCP 请求失败：已达到最大重试次数（{}次）", max_retries)
        }))
    }

    /// 内部方法：带重试逻辑的 API 调用
    ///
    /// 重试策略：
    /// - 每个凭据最多重试 MAX_RETRIES_PER_CREDENTIAL 次
    /// - 总重试次数 = min(凭据数量 × 每凭据重试次数, MAX_TOTAL_RETRIES)
    /// - 硬上限 9 次，避免无限重试
    async fn call_api_with_retry(
        &self,
        request_body: &str,
        is_stream: bool,
    ) -> anyhow::Result<ApiCallResult> {
        let total_credentials = self.token_manager.total_count();
        let max_retries = (total_credentials * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();
        let api_type = if is_stream { "流式" } else { "非流式" };

        // 尝试从请求体中提取模型信息
        let model = Self::extract_model_from_request(request_body);
        let session_key = Self::extract_conversation_id_from_request(request_body);

        for attempt in 0..max_retries {
            // 获取调用上下文（绑定 index、credentials、token）
            let ctx = match self
                .token_manager
                .acquire_context_for_session(model.as_deref(), session_key.as_deref())
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    last_error = Some(e);
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };

            let credentials = self
                .resolve_and_persist_profile_arn(
                    ctx.id,
                    &ctx.credentials,
                    &ctx.token,
                    &machine_id,
                    endpoint.as_ref(),
                )
                .await;

            let rctx = RequestContext {
                credentials: &credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.api_url(&rctx);
            let mut body = endpoint.transform_api_body(request_body, &rctx);

            if credentials.is_enterprise_auth() {
                let requested_model_id = Self::extract_model_from_request(&body);
                let codewhisperer_model_id = self
                    .resolve_codewhisperer_model_id(
                        &credentials,
                        &ctx.token,
                        &machine_id,
                        endpoint.as_ref(),
                        requested_model_id.as_deref(),
                    )
                    .await;
                body = Self::apply_payload_model_id(&body, &codewhisperer_model_id);
            }

            let base = self
                .client_for(&credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json")
                .header("Connection", "close");
            let request = endpoint.decorate_api(base, &rctx);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "API 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    // 网络错误通常是上游/链路瞬态问题，不应导致"禁用凭据"或"切换凭据"
                    // （否则一段时间网络抖动会把所有凭据都误禁用，需要重启才能恢复）
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();

            // 成功响应
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                return Ok(ApiCallResult {
                    response,
                    credential_id: ctx.id,
                });
            }

            // 失败响应：读取 body 用于日志/错误信息
            let body = response.text().await.unwrap_or_default();

            // 402 Payment Required 且额度用尽：禁用凭据并故障转移
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                tracing::warn!(
                    "API 请求失败（额度已用尽，禁用凭据并切换，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    anyhow::bail!(
                        "{} API 请求失败（所有凭据已用尽）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                continue;
            }

            // 400 Bad Request - 请求问题，重试/切换凭据无意义
            if status.as_u16() == 400 {
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 401/403 - 更可能是凭据/权限问题：计入失败并允许故障转移
            if matches!(status.as_u16(), 401 | 403) {
                if endpoint.is_account_suspended(&body) {
                    tracing::error!(
                        "凭据 #{} 已被 Kiro 官方暂停/封禁: {} {}",
                        ctx.id,
                        status,
                        body
                    );
                    let has_available = self.token_manager.report_account_suspended(ctx.id);
                    if !has_available {
                        anyhow::bail!(
                            "{} API 请求失败（所有凭据已用尽）: {} {}",
                            api_type,
                            status,
                            body
                        );
                    }
                    last_error = Some(anyhow::anyhow!(
                        "{} API 请求失败: {} {}",
                        api_type,
                        status,
                        body
                    ));
                    continue;
                }

                tracing::warn!(
                    "API 请求失败（可能为凭据错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self
                        .token_manager
                        .force_refresh_token_for(ctx.id)
                        .await
                        .is_ok()
                    {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                if !has_available {
                    anyhow::bail!(
                        "{} API 请求失败（所有凭据已用尽）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                continue;
            }

            // 429/408/5xx - 瞬态上游错误：重试但不禁用或切换凭据
            // （避免 429 high traffic / 502 high load 等瞬态错误把所有凭据锁死）
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                tracing::warn!(
                    "API 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx - 通常为请求/配置问题：直接返回，不计入凭据失败
            if status.is_client_error() {
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 兜底：当作可重试的瞬态错误处理（不切换凭据）
            tracing::warn!(
                "API 请求失败（未知错误，尝试 {}/{}）: {} {}",
                attempt + 1,
                max_retries,
                status,
                body
            );
            last_error = Some(anyhow::anyhow!(
                "{} API 请求失败: {} {}",
                api_type,
                status,
                body
            ));
            if attempt + 1 < max_retries {
                sleep(Self::retry_delay(attempt)).await;
            }
        }

        // 所有重试都失败
        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!(
                "{} API 请求失败：已达到最大重试次数（{}次）",
                api_type,
                max_retries
            )
        }))
    }

    /// 从请求体中提取模型信息
    ///
    /// 尝试解析 JSON 请求体，优先取 currentMessage 的 modelId，缺失时按 KAM 逻辑回退到 history。
    fn extract_model_from_request(request_body: &str) -> Option<String> {
        use serde_json::Value;

        let json: Value = serde_json::from_str(request_body).ok()?;

        if let Some(model_id) = json
            .get("conversationState")?
            .get("currentMessage")?
            .get("userInputMessage")?
            .get("modelId")?
            .as_str()
            .map(|s| s.to_string())
        {
            return Some(model_id);
        }

        json.get("conversationState")?
            .get("history")?
            .as_array()?
            .iter()
            .find_map(|message| {
                message
                    .get("userInputMessage")?
                    .get("modelId")?
                    .as_str()
                    .map(|s| s.to_string())
            })
    }

    fn normalize_model_key(value: &str) -> String {
        value
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect()
    }

    fn model_tokens(value: &str) -> Vec<String> {
        value
            .to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(ToString::to_string)
            .collect()
    }

    fn matches_requested_model(model: &KiroAvailableModel, requested_model_id: &str) -> bool {
        let requested_key = Self::normalize_model_key(requested_model_id);
        let model_id_key = Self::normalize_model_key(&model.model_id);
        if model_id_key == requested_key || model_id_key.contains(&requested_key) {
            return true;
        }

        if model
            .model_name
            .as_deref()
            .is_some_and(|name| Self::normalize_model_key(name).contains(&requested_key))
        {
            return true;
        }

        let tokens: Vec<String> = Self::model_tokens(requested_model_id)
            .into_iter()
            .filter(|token| token != "latest" && token != "model")
            .collect();
        if tokens.is_empty() {
            return false;
        }

        let candidate_tokens: HashSet<String> = Self::model_tokens(&format!(
            "{} {}",
            model.model_id,
            model.model_name.as_deref().unwrap_or("")
        ))
        .into_iter()
        .collect();

        if !tokens
            .iter()
            .all(|token| candidate_tokens.contains(token.as_str()))
        {
            return false;
        }

        for family in ["opus", "sonnet", "haiku"] {
            let requested_has_family = tokens.iter().any(|token| token == family);
            let candidate_has_family = candidate_tokens.contains(family);
            if requested_has_family != candidate_has_family {
                return false;
            }
        }

        true
    }

    fn is_codewhisperer_model_id(model_id: &str) -> bool {
        model_id.contains('_')
            && model_id
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    }

    fn normalize_claude_version(model_id: &str) -> String {
        let lower = model_id.trim().to_ascii_lowercase();
        let base = lower.strip_suffix("-thinking").unwrap_or(&lower);
        let parts: Vec<&str> = base.split('-').collect();

        if parts.len() < 3 || parts[0] != "claude" {
            return lower;
        }

        let family = parts[1];
        if !matches!(family, "sonnet" | "haiku" | "opus") {
            return lower;
        }

        let major = parts[2];
        if major.is_empty() || !major.chars().all(|c| c.is_ascii_digit()) {
            return lower;
        }

        if parts.len() >= 4
            && (1..=2).contains(&parts[3].len())
            && parts[3].chars().all(|c| c.is_ascii_digit())
        {
            return format!("claude-{family}-{major}.{}", parts[3]);
        }

        lower
    }

    fn fallback_kiro_model_id(requested_model_id: Option<&str>) -> String {
        let Some(model_id) = requested_model_id
            .map(str::trim)
            .filter(|model| !model.is_empty())
        else {
            return KIRO_DEFAULT_MODEL_ID.to_string();
        };

        if Self::is_codewhisperer_model_id(model_id) {
            return model_id.to_string();
        }

        let model_id = Self::normalize_claude_version(model_id);
        let lower = model_id.to_ascii_lowercase();

        match lower.as_str() {
            "claude-sonnet-4-5" | "claude-sonnet-4.5" => "claude-sonnet-4.5".to_string(),
            "claude-haiku-4-5" | "claude-haiku-4.5" => "claude-haiku-4.5".to_string(),
            "claude-opus-4-5" | "claude-opus-4.5" => "claude-opus-4.5".to_string(),
            "claude-sonnet-4" | "claude-sonnet-4-20250514" => "claude-sonnet-4".to_string(),
            "claude-3-5-sonnet" | "claude-3-opus" | "gpt-4" | "gpt-4o" | "gpt-4-turbo"
            | "gpt-3.5-turbo" => KIRO_DEFAULT_MODEL_ID.to_string(),
            "claude-3-sonnet" => "claude-sonnet-4".to_string(),
            "claude-3-haiku" => "claude-haiku-4.5".to_string(),
            _ if lower.starts_with("claude-sonnet-")
                || lower.starts_with("claude-haiku-")
                || lower.starts_with("claude-opus-") =>
            {
                model_id
            }
            _ => KIRO_DEFAULT_MODEL_ID.to_string(),
        }
    }

    async fn resolve_codewhisperer_model_id(
        &self,
        credentials: &KiroCredentials,
        token: &str,
        machine_id: &str,
        endpoint: &dyn KiroEndpoint,
        requested_model_id: Option<&str>,
    ) -> String {
        let Some(model_id) = requested_model_id
            .map(str::trim)
            .filter(|model| !model.is_empty())
        else {
            return Self::fallback_kiro_model_id(None);
        };

        if Self::is_codewhisperer_model_id(model_id) {
            return model_id.to_string();
        }
        let fallback_model_id = Self::fallback_kiro_model_id(Some(model_id));

        let cached_models = {
            let cache = self.model_cache.lock();
            cache
                .as_ref()
                .filter(|cache| cache.fetched_at.elapsed() <= MODEL_CACHE_TTL)
                .map(|cache| cache.models.clone())
        };

        let models = match cached_models {
            Some(models) => models,
            None => match self
                .fetch_available_models_for(credentials, token, machine_id, endpoint)
                .await
            {
                Ok(models) => {
                    *self.model_cache.lock() = Some(ModelCache {
                        models: models.clone(),
                        fetched_at: Instant::now(),
                    });
                    models
                }
                Err(err) => {
                    tracing::warn!(
                        "解析 CodeWhisperer modelId 时获取模型列表失败，使用请求模型回退: {}",
                        err
                    );
                    return fallback_model_id;
                }
            },
        };

        models
            .iter()
            .find(|model| Self::matches_requested_model(model, &fallback_model_id))
            .map(|model| model.model_id.clone())
            .unwrap_or(fallback_model_id)
    }

    fn apply_payload_model_id(request_body: &str, model_id: &str) -> String {
        let Ok(mut json) = serde_json::from_str::<serde_json::Value>(request_body) else {
            return request_body.to_string();
        };

        if let Some(current_model_id) =
            json.pointer_mut("/conversationState/currentMessage/userInputMessage/modelId")
        {
            *current_model_id = serde_json::Value::String(model_id.to_string());
        }

        if let Some(history) = json
            .pointer_mut("/conversationState/history")
            .and_then(|value| value.as_array_mut())
        {
            for message in history {
                if let Some(history_model_id) = message.pointer_mut("/userInputMessage/modelId") {
                    *history_model_id = serde_json::Value::String(model_id.to_string());
                }
            }
        }

        serde_json::to_string(&json).unwrap_or_else(|_| request_body.to_string())
    }

    /// 从请求体中提取 conversationId，用于 balanced 模式下的会话凭据绑定。
    fn extract_conversation_id_from_request(request_body: &str) -> Option<String> {
        use serde_json::Value;

        let json: Value = serde_json::from_str(request_body).ok()?;

        json.get("conversationState")?
            .get("conversationId")?
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    fn retry_delay(attempt: usize) -> Duration {
        // 指数退避 + 少量抖动，避免上游抖动时放大故障
        const BASE_MS: u64 = 200;
        const MAX_MS: u64 = 2_000;
        let exp = BASE_MS.saturating_mul(2u64.saturating_pow(attempt.min(6) as u32));
        let backoff = exp.min(MAX_MS);
        let jitter_max = (backoff / 4).max(1);
        let jitter = fastrand::u64(0..=jitter_max);
        Duration::from_millis(backoff.saturating_add(jitter))
    }
}

#[cfg(test)]
mod tests {
    use super::{KiroAvailableModel, KiroProvider};

    #[test]
    fn fallback_kiro_model_id_keeps_supported_kiro_model() {
        assert_eq!(
            KiroProvider::fallback_kiro_model_id(Some("claude-haiku-4.5")),
            "claude-haiku-4.5"
        );
        assert_eq!(
            KiroProvider::fallback_kiro_model_id(Some("claude-sonnet-4.6")),
            "claude-sonnet-4.6"
        );
    }

    #[test]
    fn fallback_kiro_model_id_normalizes_anthropic_snapshot_model() {
        assert_eq!(
            KiroProvider::fallback_kiro_model_id(Some("claude-haiku-4-5-20251001")),
            "claude-haiku-4.5"
        );
        assert_eq!(
            KiroProvider::fallback_kiro_model_id(Some("claude-sonnet-4-5-20250929-thinking")),
            "claude-sonnet-4.5"
        );
    }

    #[test]
    fn fallback_kiro_model_id_uses_kiro_default_for_unknown_model() {
        assert_eq!(
            KiroProvider::fallback_kiro_model_id(Some("not-a-real-model")),
            "claude-sonnet-4.5"
        );
        assert_eq!(
            KiroProvider::fallback_kiro_model_id(None),
            "claude-sonnet-4.5"
        );
    }

    #[test]
    fn fallback_kiro_model_id_preserves_explicit_codewhisperer_model() {
        assert_eq!(
            KiroProvider::fallback_kiro_model_id(Some("CLAUDE_HAIKU_4_5_20251001_V1_0")),
            "CLAUDE_HAIKU_4_5_20251001_V1_0"
        );
    }

    #[test]
    fn matches_requested_model_accepts_normalized_kiro_model() {
        let model = KiroAvailableModel {
            model_id: "claude-haiku-4.5".to_string(),
            model_name: Some("Claude Haiku 4.5".to_string()),
            description: None,
            model_provider: None,
            token_limits: None,
        };

        let normalized = KiroProvider::fallback_kiro_model_id(Some("claude-haiku-4-5-20251001"));
        assert!(KiroProvider::matches_requested_model(&model, &normalized));
    }

    #[test]
    fn apply_payload_model_id_updates_current_and_history() {
        let body = r#"{"conversationState":{"currentMessage":{"userInputMessage":{"modelId":"old"}},"history":[{"userInputMessage":{"modelId":"old-history"}},{"assistantResponseMessage":{"content":"ok"}}]}}"#;

        let updated = KiroProvider::apply_payload_model_id(body, "claude-haiku-4.5");
        let json: serde_json::Value = serde_json::from_str(&updated).unwrap();

        assert_eq!(
            json.pointer("/conversationState/currentMessage/userInputMessage/modelId"),
            Some(&serde_json::Value::String("claude-haiku-4.5".to_string()))
        );
        assert_eq!(
            json.pointer("/conversationState/history/0/userInputMessage/modelId"),
            Some(&serde_json::Value::String("claude-haiku-4.5".to_string()))
        );
    }
}
