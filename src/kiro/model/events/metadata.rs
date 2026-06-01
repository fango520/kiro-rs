//! 消息元数据事件
//!
//! 处理 messageMetadataEvent / metadataEvent 类型的事件。

use serde_json::Value;

use crate::kiro::parser::error::ParseResult;
use crate::kiro::parser::frame::Frame;

use super::base::EventPayload;

/// 消息元数据事件。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageMetadataEvent {
    /// Kiro 后端返回的真实 token 统计。
    pub token_usage: Option<TokenUsage>,
}

/// Kiro 后端 tokenUsage 统计。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenUsage {
    pub uncached_input_tokens: Option<i32>,
    pub cache_read_input_tokens: Option<i32>,
    pub cache_write_input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
    pub context_usage_percentage: Option<f64>,
}

impl TokenUsage {
    pub fn input_tokens(&self) -> Option<i32> {
        let input_from_breakdown = self
            .uncached_input_tokens
            .unwrap_or_default()
            .saturating_add(self.cache_read_input_tokens.unwrap_or_default())
            .saturating_add(self.cache_write_input_tokens.unwrap_or_default());

        if input_from_breakdown > 0 {
            return Some(input_from_breakdown);
        }

        match (self.total_tokens, self.output_tokens) {
            (Some(total), Some(output)) if total > output => Some(total - output),
            _ => None,
        }
    }

    pub fn has_input_breakdown(&self) -> bool {
        self.uncached_input_tokens.is_some()
            || self.cache_read_input_tokens.is_some()
            || self.cache_write_input_tokens.is_some()
    }

    pub fn cache_read_tokens(&self) -> i32 {
        self.cache_read_input_tokens.unwrap_or_default().max(0)
    }

    pub fn cache_write_tokens(&self) -> i32 {
        self.cache_write_input_tokens.unwrap_or_default().max(0)
    }
}

impl EventPayload for MessageMetadataEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        let value: Value = frame.payload_as_json()?;
        Ok(Self::from_value(&value))
    }
}

impl MessageMetadataEvent {
    fn from_value(value: &Value) -> Self {
        let metadata = value
            .get("messageMetadataEvent")
            .or_else(|| value.get("metadataEvent"))
            .unwrap_or(value);

        let token_usage = metadata
            .get("tokenUsage")
            .or_else(|| metadata.get("token_usage"))
            .map(TokenUsage::from_value);

        Self { token_usage }
    }
}

impl TokenUsage {
    fn from_value(value: &Value) -> Self {
        Self {
            uncached_input_tokens: get_i32(value, "uncachedInputTokens")
                .or_else(|| get_i32(value, "uncached_input_tokens")),
            cache_read_input_tokens: get_i32(value, "cacheReadInputTokens")
                .or_else(|| get_i32(value, "cache_read_input_tokens")),
            cache_write_input_tokens: get_i32(value, "cacheWriteInputTokens")
                .or_else(|| get_i32(value, "cache_write_input_tokens")),
            output_tokens: get_i32(value, "outputTokens")
                .or_else(|| get_i32(value, "output_tokens")),
            total_tokens: get_i32(value, "totalTokens").or_else(|| get_i32(value, "total_tokens")),
            context_usage_percentage: get_f64(value, "contextUsagePercentage")
                .or_else(|| get_f64(value, "context_usage_percentage")),
        }
    }
}

fn get_i32(value: &Value, key: &str) -> Option<i32> {
    let raw = value.get(key)?;
    if let Some(n) = raw.as_i64() {
        return Some(n.clamp(0, i32::MAX as i64) as i32);
    }
    if let Some(n) = raw.as_u64() {
        return Some(n.min(i32::MAX as u64) as i32);
    }
    if let Some(n) = raw.as_f64() {
        if n.is_finite() {
            return Some(n.round().clamp(0.0, i32::MAX as f64) as i32);
        }
    }
    raw.as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite())
        .map(|n| n.round().clamp(0.0, i32::MAX as f64) as i32)
}

fn get_f64(value: &Value, key: &str) -> Option<f64> {
    let raw = value.get(key)?;
    if let Some(n) = raw.as_f64() {
        return n.is_finite().then_some(n);
    }
    raw.as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_token_usage() {
        let value = serde_json::json!({
            "tokenUsage": {
                "uncachedInputTokens": 12,
                "cacheReadInputTokens": 34,
                "cacheWriteInputTokens": 56,
                "outputTokens": 7,
                "totalTokens": 109,
                "contextUsagePercentage": 1.5
            }
        });

        let event = MessageMetadataEvent::from_value(&value);
        let usage = event.token_usage.unwrap();

        assert_eq!(usage.input_tokens(), Some(102));
        assert_eq!(usage.cache_read_tokens(), 34);
        assert_eq!(usage.cache_write_tokens(), 56);
        assert_eq!(usage.output_tokens, Some(7));
        assert_eq!(usage.context_usage_percentage, Some(1.5));
    }

    #[test]
    fn parses_nested_metadata_event() {
        let value = serde_json::json!({
            "messageMetadataEvent": {
                "tokenUsage": {
                    "totalTokens": 100,
                    "outputTokens": 25
                }
            }
        });

        let event = MessageMetadataEvent::from_value(&value);
        assert_eq!(event.token_usage.unwrap().input_tokens(), Some(75));
    }
}
