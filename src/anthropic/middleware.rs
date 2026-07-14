//! Anthropic API 中间件

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use crate::common::auth;
use crate::api_keys::{ApiKeyContext, ApiKeyStore};
use crate::request_log::RequestLogStore;
use crate::kiro::provider::KiroProvider;

use super::cache_tracker::CacheTracker;
use super::types::ErrorResponse;

#[derive(Clone)]
pub(crate) struct PromptCacheSnapshot {
    pub accounting_enabled: bool,
    pub ttl_seconds: u64,
    pub tracker: Arc<CacheTracker>,
}

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// API Key 管理器
    pub api_key_store: ApiKeyStore,
    /// 请求日志
    pub request_log_store: RequestLogStore,
    /// Kiro Provider（可选，用于实际 API 调用）
    /// 内部使用 MultiTokenManager，已支持线程安全的多凭据管理
    pub kiro_provider: Option<Arc<KiroProvider>>,
    /// 是否开启非流式响应的 thinking 块提取
    pub extract_thinking: bool,
    /// 本地 Prompt Cache usage 记账快照
    pub prompt_cache: PromptCacheSnapshot,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(
        api_key_store: ApiKeyStore,
        request_log_store: RequestLogStore,
        extract_thinking: bool,
        prompt_cache_ttl_seconds: u64,
        prompt_cache_accounting_enabled: bool,
    ) -> Self {
        Self {
            api_key_store,
            request_log_store,
            kiro_provider: None,
            extract_thinking,
            prompt_cache: PromptCacheSnapshot {
                accounting_enabled: prompt_cache_accounting_enabled,
                ttl_seconds: prompt_cache_ttl_seconds,
                tracker: Arc::new(CacheTracker::new(Duration::from_secs(
                    prompt_cache_ttl_seconds,
                ))),
            },
        }
    }

    /// 设置 KiroProvider
    pub fn with_kiro_provider(mut self, provider: KiroProvider) -> Self {
        self.kiro_provider = Some(Arc::new(provider));
        self
    }

    pub fn prompt_cache_snapshot(&self) -> PromptCacheSnapshot {
        self.prompt_cache.clone()
    }
}

/// API Key 认证中间件
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    match auth::extract_api_key(&request) {
        Some(key) => {
            if let Some(ctx) = state.api_key_store.validate(&key) {
                request.extensions_mut().insert::<ApiKeyContext>(ctx);
                next.run(request).await
            } else {
                let error = ErrorResponse::authentication_error();
                (StatusCode::UNAUTHORIZED, Json(error)).into_response()
            }
        }
        _ => {
            let error = ErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}

/// CORS 中间件层
///
/// **安全说明**：当前配置允许所有来源（Any），这是为了支持公开 API 服务。
/// 如果需要更严格的安全控制，请根据实际需求配置具体的允许来源、方法和头信息。
///
/// # 配置说明
/// - `allow_origin(Any)`: 允许任何来源的请求
/// - `allow_methods(Any)`: 允许任何 HTTP 方法
/// - `allow_headers(Any)`: 允许任何请求头
pub fn cors_layer() -> tower_http::cors::CorsLayer {
    use tower_http::cors::{Any, CorsLayer};

    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}
