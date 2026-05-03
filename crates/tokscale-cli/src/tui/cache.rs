//! TUI data caching for instant startup.
//!
//! This module provides disk-based caching for TUI data to enable instant UI display
//! while fresh data loads in the background (matching TypeScript implementation behavior).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokscale_core::{sessions, GroupBy};

use crate::ClientFilter;

use super::data::{
    AgentUsage, CodexAccountUsage, ContributionDay, DailyModelInfo, DailySourceInfo, DailyUsage,
    GraphData, HourlyModelInfo, HourlyUsage, ModelUsage, PriceSummary, PriceUsage, QuotaConfidence,
    QuotaModelSummary, QuotaValueData, QuotaValueInterval, QuotaValuePoint, SpeedSummary,
    SpeedUsage, ThinkingSummary, ThinkingUsage, TokenBreakdown, UsageData,
};

/// Cache staleness threshold for automatic background refresh.
/// TUI startup should prefer showing cached data immediately; explicit refresh
/// and configured auto-refresh still force a reload when the user wants it.
const CACHE_STALE_THRESHOLD_MS: u64 = 60 * 60 * 1000;
const CACHE_SCHEMA_VERSION: u32 = 17;

/// Get the cache directory path
/// Uses `~/.cache/tokscale/` to match TypeScript implementation for cache sharing
fn cache_dir() -> Option<PathBuf> {
    Some(crate::paths::get_cache_dir())
}

/// Get the cache file path
fn cache_file() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("tui-data-cache.json"))
}

fn legacy_cache_files() -> Vec<PathBuf> {
    if crate::paths::is_config_dir_overridden() {
        return Vec::new();
    }

    crate::paths::legacy_dot_cache_tokscale_dir()
        .map(|dir| vec![dir.join("tui-data-cache.json")])
        .unwrap_or_default()
}

/// Cached TUI data structure (serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedTUIData {
    #[serde(default)]
    schema_version: u32,
    timestamp: u64,
    enabled_clients: Vec<String>,
    #[serde(default)]
    include_synthetic: bool,
    #[serde(default)]
    group_by: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    year: Option<String>,
    data: CachedUsageData,
}

/// Serializable version of UsageData
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedUsageData {
    models: Vec<CachedModelUsage>,
    #[serde(default)]
    agents: Vec<CachedAgentUsage>,
    daily: Vec<CachedDailyUsage>,
    #[serde(default)]
    hourly: Vec<CachedHourlyUsage>,
    #[serde(default)]
    prices: Vec<CachedPriceUsage>,
    #[serde(default)]
    thinking: Vec<CachedThinkingUsage>,
    #[serde(default)]
    speeds: Vec<CachedSpeedUsage>,
    #[serde(default)]
    codex_accounts: Vec<CachedCodexAccountUsage>,
    #[serde(default)]
    quota_value: CachedQuotaValueData,
    graph: Option<CachedGraphData>,
    total_tokens: u64,
    total_cost: f64,
    current_streak: u32,
    longest_streak: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedTokenBreakdown {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedModelUsage {
    model: String,
    provider: String,
    client: String,
    #[serde(default)]
    workspace_key: Option<String>,
    #[serde(default)]
    workspace_label: Option<String>,
    tokens: CachedTokenBreakdown,
    cost: f64,
    session_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedAgentUsage {
    agent: String,
    clients: String,
    tokens: CachedTokenBreakdown,
    cost: f64,
    message_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedDailyModelInfo {
    #[serde(default)]
    client: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    color_key: String,
    tokens: CachedTokenBreakdown,
    cost: f64,
    #[serde(default)]
    messages: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedDailySourceInfo {
    tokens: CachedTokenBreakdown,
    cost: f64,
    models: Vec<(String, CachedDailyModelInfo)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedDailyUsage {
    date: String, // NaiveDate serialized as string
    tokens: CachedTokenBreakdown,
    cost: f64,
    #[serde(default)]
    models: Vec<(String, CachedDailyModelInfo)>,
    #[serde(default)]
    source_breakdown: Vec<(String, CachedDailySourceInfo)>,
    #[serde(default)]
    message_count: u32,
    #[serde(default)]
    turn_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedHourlyModelInfo {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    color_key: String,
    tokens: CachedTokenBreakdown,
    cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedHourlyUsage {
    datetime: String, // NaiveDateTime as "YYYY-MM-DD HH:MM:SS"
    tokens: CachedTokenBreakdown,
    cost: f64,
    clients: Vec<String>,
    models: Vec<(String, CachedHourlyModelInfo)>,
    #[serde(default)]
    message_count: u32,
    #[serde(default)]
    turn_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedPriceUsage {
    date: String,
    model: String,
    provider: String,
    #[serde(default)]
    pricing_source: Option<String>,
    #[serde(default)]
    matched_key: Option<String>,
    tokens: CachedTokenBreakdown,
    cost: f64,
    #[serde(default)]
    clients: Vec<String>,
    #[serde(default)]
    message_count: u32,
    #[serde(default)]
    input_price_per_million: Option<f64>,
    #[serde(default)]
    output_price_per_million: Option<f64>,
    #[serde(default)]
    cache_read_price_per_million: Option<f64>,
    #[serde(default)]
    cache_write_price_per_million: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedThinkingUsage {
    date: String,
    model: String,
    provider: String,
    thinking_level: String,
    tokens: CachedTokenBreakdown,
    cost: f64,
    #[serde(default)]
    clients: Vec<String>,
    #[serde(default)]
    message_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedSpeedUsage {
    date: String,
    model: String,
    provider: String,
    thinking_level: String,
    generated_tokens: u64,
    generation_duration_ms: u64,
    #[serde(default)]
    clients: Vec<String>,
    sample_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedCodexAccountUsage {
    account_hash: String,
    tokens: CachedTokenBreakdown,
    cost: f64,
    #[serde(default)]
    paid_cost: Option<f64>,
    #[serde(default)]
    active_month_count: Option<u32>,
    message_count: u32,
    turn_count: u32,
    session_count: u32,
    #[serde(default)]
    first_date: Option<String>,
    #[serde(default)]
    latest_date: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedQuotaValueData {
    #[serde(default)]
    intervals: Vec<CachedQuotaValueInterval>,
    #[serde(default)]
    points: Vec<CachedQuotaValuePoint>,
    #[serde(default)]
    model_summaries: Vec<CachedQuotaModelSummary>,
    #[serde(default)]
    sample_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedQuotaValueInterval {
    account_hash: String,
    #[serde(default)]
    model: String,
    window_kind: String,
    end: String,
    quota_burn_pct: f64,
    api_value_usd: f64,
    subscription_cost_burned: f64,
    factor: Option<f64>,
    dollars_per_percent: Option<f64>,
    tokens: CachedTokenBreakdown,
    models: Vec<String>,
    sample_count: u32,
    confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedQuotaValuePoint {
    date: String,
    #[serde(default)]
    model: String,
    window_kind: String,
    quota_burn_pct: f64,
    api_value_usd: f64,
    subscription_cost_burned: f64,
    factor: Option<f64>,
    dollars_per_percent: Option<f64>,
    interval_count: u32,
    confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedQuotaModelSummary {
    model: String,
    window_kind: String,
    latest_date: String,
    quota_burn_pct: f64,
    api_value_usd: f64,
    subscription_cost_burned: f64,
    factor: Option<f64>,
    dollars_per_percent: Option<f64>,
    tokens: CachedTokenBreakdown,
    interval_count: u32,
    confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedContributionDay {
    date: String,
    tokens: u64,
    cost: f64,
    intensity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedGraphData {
    weeks: Vec<Vec<Option<CachedContributionDay>>>,
}

// Conversion implementations

impl From<&TokenBreakdown> for CachedTokenBreakdown {
    fn from(t: &TokenBreakdown) -> Self {
        Self {
            input: t.input,
            output: t.output,
            cache_read: t.cache_read,
            cache_write: t.cache_write,
            reasoning: t.reasoning,
        }
    }
}

impl From<CachedTokenBreakdown> for TokenBreakdown {
    fn from(t: CachedTokenBreakdown) -> Self {
        Self {
            input: t.input,
            output: t.output,
            cache_read: t.cache_read,
            cache_write: t.cache_write,
            reasoning: t.reasoning,
        }
    }
}

impl From<&ModelUsage> for CachedModelUsage {
    fn from(m: &ModelUsage) -> Self {
        Self {
            model: m.model.clone(),
            provider: m.provider.clone(),
            client: m.client.clone(),
            workspace_key: m.workspace_key.clone(),
            workspace_label: m.workspace_label.clone(),
            tokens: (&m.tokens).into(),
            cost: m.cost,
            session_count: m.session_count,
        }
    }
}

impl From<CachedModelUsage> for ModelUsage {
    fn from(m: CachedModelUsage) -> Self {
        Self {
            model: m.model,
            provider: m.provider,
            client: m.client,
            workspace_key: m.workspace_key,
            workspace_label: m.workspace_label,
            tokens: m.tokens.into(),
            cost: m.cost,
            session_count: m.session_count,
        }
    }
}

impl From<&AgentUsage> for CachedAgentUsage {
    fn from(a: &AgentUsage) -> Self {
        Self {
            agent: a.agent.clone(),
            clients: a.clients.clone(),
            tokens: (&a.tokens).into(),
            cost: a.cost,
            message_count: a.message_count,
        }
    }
}

impl From<CachedAgentUsage> for AgentUsage {
    fn from(a: CachedAgentUsage) -> Self {
        Self {
            agent: a.agent,
            clients: a.clients,
            tokens: a.tokens.into(),
            cost: a.cost,
            message_count: a.message_count,
        }
    }
}

impl From<&DailyModelInfo> for CachedDailyModelInfo {
    fn from(d: &DailyModelInfo) -> Self {
        Self {
            client: String::new(),
            provider: d.provider.clone(),
            display_name: d.display_name.clone(),
            color_key: d.color_key.clone(),
            tokens: (&d.tokens).into(),
            cost: d.cost,
            messages: d.messages,
        }
    }
}

fn daily_model_info_from_cached(key: &str, value: CachedDailyModelInfo) -> DailyModelInfo {
    let display_name = if value.display_name.is_empty() {
        key.to_string()
    } else {
        value.display_name
    };
    let color_key = if value.color_key.is_empty() {
        display_name
            .rsplit_once(" / ")
            .map(|(_, base_model)| base_model.to_string())
            .unwrap_or_else(|| display_name.clone())
    } else {
        value.color_key
    };

    DailyModelInfo {
        provider: value.provider,
        display_name,
        color_key,
        tokens: value.tokens.into(),
        cost: value.cost,
        messages: value.messages,
    }
}

impl From<&DailySourceInfo> for CachedDailySourceInfo {
    fn from(source: &DailySourceInfo) -> Self {
        Self {
            tokens: (&source.tokens).into(),
            cost: source.cost,
            models: source
                .models
                .iter()
                .map(|(key, value)| (key.clone(), value.into()))
                .collect(),
        }
    }
}

impl From<CachedDailySourceInfo> for DailySourceInfo {
    fn from(source: CachedDailySourceInfo) -> Self {
        Self {
            tokens: source.tokens.into(),
            cost: source.cost,
            models: source
                .models
                .into_iter()
                .map(|(key, value)| {
                    let model_info = daily_model_info_from_cached(&key, value);
                    (key, model_info)
                })
                .collect(),
        }
    }
}

impl From<&HourlyModelInfo> for CachedHourlyModelInfo {
    fn from(h: &HourlyModelInfo) -> Self {
        Self {
            provider: h.provider.clone(),
            display_name: h.display_name.clone(),
            color_key: h.color_key.clone(),
            tokens: (&h.tokens).into(),
            cost: h.cost,
        }
    }
}

fn hourly_model_info_from_cached(key: &str, value: CachedHourlyModelInfo) -> HourlyModelInfo {
    let display_name = if value.display_name.is_empty() {
        key.to_string()
    } else {
        value.display_name
    };
    let color_key = if value.color_key.is_empty() {
        display_name
            .rsplit_once(" / ")
            .map(|(_, base_model)| base_model.to_string())
            .unwrap_or_else(|| display_name.clone())
    } else {
        value.color_key
    };

    HourlyModelInfo {
        provider: value.provider,
        display_name,
        color_key,
        tokens: value.tokens.into(),
        cost: value.cost,
    }
}

impl From<&HourlyUsage> for CachedHourlyUsage {
    fn from(h: &HourlyUsage) -> Self {
        Self {
            datetime: h.datetime.format("%Y-%m-%d %H:%M:%S").to_string(),
            tokens: (&h.tokens).into(),
            cost: h.cost,
            clients: h.clients.iter().cloned().collect(),
            models: h
                .models
                .iter()
                .map(|(k, v)| (k.clone(), v.into()))
                .collect(),
            message_count: h.message_count,
            turn_count: h.turn_count,
        }
    }
}

impl From<&PriceUsage> for CachedPriceUsage {
    fn from(p: &PriceUsage) -> Self {
        Self {
            date: p.date.to_string(),
            model: p.model.clone(),
            provider: p.provider.clone(),
            pricing_source: p.pricing_source.clone(),
            matched_key: p.matched_key.clone(),
            tokens: (&p.tokens).into(),
            cost: p.cost,
            clients: p.clients.iter().cloned().collect(),
            message_count: p.message_count,
            input_price_per_million: p.input_price_per_million,
            output_price_per_million: p.output_price_per_million,
            cache_read_price_per_million: p.cache_read_price_per_million,
            cache_write_price_per_million: p.cache_write_price_per_million,
        }
    }
}

impl TryFrom<CachedPriceUsage> for PriceUsage {
    type Error = chrono::ParseError;

    fn try_from(p: CachedPriceUsage) -> Result<Self, Self::Error> {
        use chrono::NaiveDate;

        Ok(Self {
            date: NaiveDate::parse_from_str(&p.date, "%Y-%m-%d")?,
            model: p.model,
            provider: p.provider,
            pricing_source: p.pricing_source,
            matched_key: p.matched_key,
            tokens: p.tokens.into(),
            cost: p.cost,
            clients: p.clients.into_iter().collect(),
            message_count: p.message_count,
            input_price_per_million: p.input_price_per_million,
            output_price_per_million: p.output_price_per_million,
            cache_read_price_per_million: p.cache_read_price_per_million,
            cache_write_price_per_million: p.cache_write_price_per_million,
        })
    }
}

impl From<&ThinkingUsage> for CachedThinkingUsage {
    fn from(t: &ThinkingUsage) -> Self {
        Self {
            date: t.date.to_string(),
            model: t.model.clone(),
            provider: t.provider.clone(),
            thinking_level: t.thinking_level.clone(),
            tokens: (&t.tokens).into(),
            cost: t.cost,
            clients: t.clients.iter().cloned().collect(),
            message_count: t.message_count,
        }
    }
}

impl TryFrom<CachedThinkingUsage> for ThinkingUsage {
    type Error = chrono::ParseError;

    fn try_from(t: CachedThinkingUsage) -> Result<Self, Self::Error> {
        use chrono::NaiveDate;

        Ok(Self {
            date: NaiveDate::parse_from_str(&t.date, "%Y-%m-%d")?,
            model: t.model,
            provider: t.provider,
            thinking_level: t.thinking_level,
            tokens: t.tokens.into(),
            cost: t.cost,
            clients: t.clients.into_iter().collect(),
            message_count: t.message_count,
        })
    }
}

impl From<&SpeedUsage> for CachedSpeedUsage {
    fn from(s: &SpeedUsage) -> Self {
        Self {
            date: s.date.to_string(),
            model: s.model.clone(),
            provider: s.provider.clone(),
            thinking_level: s.thinking_level.clone(),
            generated_tokens: s.generated_tokens,
            generation_duration_ms: s.generation_duration_ms,
            clients: s.clients.iter().cloned().collect(),
            sample_count: s.sample_count,
        }
    }
}

impl TryFrom<CachedSpeedUsage> for SpeedUsage {
    type Error = chrono::ParseError;

    fn try_from(s: CachedSpeedUsage) -> Result<Self, Self::Error> {
        use chrono::NaiveDate;

        Ok(Self {
            date: NaiveDate::parse_from_str(&s.date, "%Y-%m-%d")?,
            model: s.model,
            provider: s.provider,
            thinking_level: s.thinking_level,
            generated_tokens: s.generated_tokens,
            generation_duration_ms: s.generation_duration_ms,
            clients: s.clients.into_iter().collect(),
            sample_count: s.sample_count,
        })
    }
}

impl From<&CodexAccountUsage> for CachedCodexAccountUsage {
    fn from(a: &CodexAccountUsage) -> Self {
        Self {
            account_hash: a.account_hash.clone(),
            tokens: (&a.tokens).into(),
            cost: a.cost,
            paid_cost: a.paid_cost,
            active_month_count: a.active_month_count,
            message_count: a.message_count,
            turn_count: a.turn_count,
            session_count: a.session_count,
            first_date: a.first_date.map(|date| date.to_string()),
            latest_date: a.latest_date.map(|date| date.to_string()),
        }
    }
}

impl TryFrom<CachedCodexAccountUsage> for CodexAccountUsage {
    type Error = chrono::ParseError;

    fn try_from(a: CachedCodexAccountUsage) -> Result<Self, Self::Error> {
        use chrono::NaiveDate;

        Ok(Self {
            account_hash: a.account_hash,
            tokens: a.tokens.into(),
            cost: a.cost,
            paid_cost: a.paid_cost,
            active_month_count: a.active_month_count,
            message_count: a.message_count,
            turn_count: a.turn_count,
            session_count: a.session_count,
            first_date: a
                .first_date
                .map(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d"))
                .transpose()?,
            latest_date: a
                .latest_date
                .map(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d"))
                .transpose()?,
        })
    }
}

impl TryFrom<CachedHourlyUsage> for HourlyUsage {
    type Error = chrono::ParseError;

    fn try_from(h: CachedHourlyUsage) -> Result<Self, Self::Error> {
        use chrono::NaiveDateTime;
        Ok(Self {
            datetime: NaiveDateTime::parse_from_str(&h.datetime, "%Y-%m-%d %H:%M:%S")?,
            tokens: h.tokens.into(),
            cost: h.cost,
            clients: h.clients.into_iter().collect(),
            models: h
                .models
                .into_iter()
                .map(|(key, value)| {
                    let model_info = hourly_model_info_from_cached(&key, value);
                    (key, model_info)
                })
                .collect(),
            message_count: h.message_count,
            turn_count: h.turn_count,
        })
    }
}

impl From<&QuotaValueData> for CachedQuotaValueData {
    fn from(data: &QuotaValueData) -> Self {
        Self {
            intervals: data
                .intervals
                .iter()
                .map(|interval| interval.into())
                .collect(),
            points: data.points.iter().map(|point| point.into()).collect(),
            model_summaries: data
                .model_summaries
                .iter()
                .map(|summary| summary.into())
                .collect(),
            sample_count: data.sample_count,
        }
    }
}

impl TryFrom<CachedQuotaValueData> for QuotaValueData {
    type Error = chrono::ParseError;

    fn try_from(data: CachedQuotaValueData) -> Result<Self, Self::Error> {
        let intervals: Result<Vec<QuotaValueInterval>, _> = data
            .intervals
            .into_iter()
            .map(|row| row.try_into())
            .collect();
        let points: Result<Vec<QuotaValuePoint>, _> = data
            .points
            .into_iter()
            .map(|point| point.try_into())
            .collect();
        let model_summaries: Result<Vec<QuotaModelSummary>, _> = data
            .model_summaries
            .into_iter()
            .map(|summary| summary.try_into())
            .collect();
        Ok(Self {
            intervals: intervals?,
            points: points?,
            model_summaries: model_summaries?,
            sample_count: data.sample_count,
        })
    }
}

impl From<&QuotaValueInterval> for CachedQuotaValueInterval {
    fn from(interval: &QuotaValueInterval) -> Self {
        Self {
            account_hash: interval.account_hash.clone(),
            model: interval.model.clone(),
            window_kind: interval.window_kind.clone(),
            end: interval.end.to_string(),
            quota_burn_pct: interval.quota_burn_pct,
            api_value_usd: interval.api_value_usd,
            subscription_cost_burned: interval.subscription_cost_burned,
            factor: interval.factor,
            dollars_per_percent: interval.dollars_per_percent,
            tokens: (&interval.tokens).into(),
            models: interval.models.iter().cloned().collect(),
            sample_count: interval.sample_count,
            confidence: interval.confidence.as_str().to_string(),
        }
    }
}

impl TryFrom<CachedQuotaValueInterval> for QuotaValueInterval {
    type Error = chrono::ParseError;

    fn try_from(interval: CachedQuotaValueInterval) -> Result<Self, Self::Error> {
        Ok(Self {
            account_hash: interval.account_hash,
            model: interval.model,
            window_kind: interval.window_kind,
            end: chrono::NaiveDateTime::parse_from_str(&interval.end, "%Y-%m-%d %H:%M:%S")?,
            quota_burn_pct: interval.quota_burn_pct,
            api_value_usd: interval.api_value_usd,
            subscription_cost_burned: interval.subscription_cost_burned,
            factor: interval.factor,
            dollars_per_percent: interval.dollars_per_percent,
            tokens: interval.tokens.into(),
            models: interval.models.into_iter().collect(),
            sample_count: interval.sample_count,
            confidence: parse_quota_confidence(&interval.confidence),
        })
    }
}

impl From<&QuotaValuePoint> for CachedQuotaValuePoint {
    fn from(point: &QuotaValuePoint) -> Self {
        Self {
            date: point.date.to_string(),
            model: point.model.clone(),
            window_kind: point.window_kind.clone(),
            quota_burn_pct: point.quota_burn_pct,
            api_value_usd: point.api_value_usd,
            subscription_cost_burned: point.subscription_cost_burned,
            factor: point.factor,
            dollars_per_percent: point.dollars_per_percent,
            interval_count: point.interval_count,
            confidence: point.confidence.as_str().to_string(),
        }
    }
}

impl TryFrom<CachedQuotaValuePoint> for QuotaValuePoint {
    type Error = chrono::ParseError;

    fn try_from(point: CachedQuotaValuePoint) -> Result<Self, Self::Error> {
        Ok(Self {
            date: chrono::NaiveDate::parse_from_str(&point.date, "%Y-%m-%d")?,
            model: point.model,
            window_kind: point.window_kind,
            quota_burn_pct: point.quota_burn_pct,
            api_value_usd: point.api_value_usd,
            subscription_cost_burned: point.subscription_cost_burned,
            factor: point.factor,
            dollars_per_percent: point.dollars_per_percent,
            interval_count: point.interval_count,
            confidence: parse_quota_confidence(&point.confidence),
        })
    }
}

impl From<&QuotaModelSummary> for CachedQuotaModelSummary {
    fn from(summary: &QuotaModelSummary) -> Self {
        Self {
            model: summary.model.clone(),
            window_kind: summary.window_kind.clone(),
            latest_date: summary.latest_date.to_string(),
            quota_burn_pct: summary.quota_burn_pct,
            api_value_usd: summary.api_value_usd,
            subscription_cost_burned: summary.subscription_cost_burned,
            factor: summary.factor,
            dollars_per_percent: summary.dollars_per_percent,
            tokens: (&summary.tokens).into(),
            interval_count: summary.interval_count,
            confidence: summary.confidence.as_str().to_string(),
        }
    }
}

impl TryFrom<CachedQuotaModelSummary> for QuotaModelSummary {
    type Error = chrono::ParseError;

    fn try_from(summary: CachedQuotaModelSummary) -> Result<Self, Self::Error> {
        Ok(Self {
            model: summary.model,
            window_kind: summary.window_kind,
            latest_date: chrono::NaiveDate::parse_from_str(&summary.latest_date, "%Y-%m-%d")?,
            quota_burn_pct: summary.quota_burn_pct,
            api_value_usd: summary.api_value_usd,
            subscription_cost_burned: summary.subscription_cost_burned,
            factor: summary.factor,
            dollars_per_percent: summary.dollars_per_percent,
            tokens: summary.tokens.into(),
            interval_count: summary.interval_count,
            confidence: parse_quota_confidence(&summary.confidence),
        })
    }
}

fn parse_quota_confidence(value: &str) -> QuotaConfidence {
    match value {
        "High" => QuotaConfidence::High,
        "Medium" => QuotaConfidence::Medium,
        _ => QuotaConfidence::Low,
    }
}

impl From<&DailyUsage> for CachedDailyUsage {
    fn from(d: &DailyUsage) -> Self {
        Self {
            date: d.date.to_string(),
            tokens: (&d.tokens).into(),
            cost: d.cost,
            models: Vec::new(),
            source_breakdown: d
                .source_breakdown
                .iter()
                .map(|(key, value)| (key.clone(), value.into()))
                .collect(),
            message_count: d.message_count,
            turn_count: d.turn_count,
        }
    }
}

impl TryFrom<CachedDailyUsage> for DailyUsage {
    type Error = chrono::ParseError;

    fn try_from(d: CachedDailyUsage) -> Result<Self, Self::Error> {
        use chrono::NaiveDate;

        let source_breakdown = if d.source_breakdown.is_empty() {
            let mut legacy_sources: BTreeMap<String, DailySourceInfo> = BTreeMap::new();
            for (key, value) in d.models {
                let client = if value.client.is_empty() {
                    "unknown".to_string()
                } else {
                    value.client.clone()
                };
                let model_info = daily_model_info_from_cached(&key, value);
                let source = legacy_sources
                    .entry(client)
                    .or_insert_with(|| DailySourceInfo {
                        tokens: TokenBreakdown::default(),
                        cost: 0.0,
                        models: BTreeMap::new(),
                    });
                source.tokens.input = source.tokens.input.saturating_add(model_info.tokens.input);
                source.tokens.output = source
                    .tokens
                    .output
                    .saturating_add(model_info.tokens.output);
                source.tokens.cache_read = source
                    .tokens
                    .cache_read
                    .saturating_add(model_info.tokens.cache_read);
                source.tokens.cache_write = source
                    .tokens
                    .cache_write
                    .saturating_add(model_info.tokens.cache_write);
                source.tokens.reasoning = source
                    .tokens
                    .reasoning
                    .saturating_add(model_info.tokens.reasoning);
                source.cost += model_info.cost;
                source.models.insert(key, model_info);
            }
            legacy_sources
        } else {
            d.source_breakdown
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect()
        };

        Ok(Self {
            date: NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")?,
            tokens: d.tokens.into(),
            cost: d.cost,
            source_breakdown,
            message_count: d.message_count,
            turn_count: d.turn_count,
        })
    }
}

impl From<&ContributionDay> for CachedContributionDay {
    fn from(c: &ContributionDay) -> Self {
        Self {
            date: c.date.to_string(),
            tokens: c.tokens,
            cost: c.cost,
            intensity: c.intensity,
        }
    }
}

impl TryFrom<CachedContributionDay> for ContributionDay {
    type Error = chrono::ParseError;

    fn try_from(c: CachedContributionDay) -> Result<Self, Self::Error> {
        use chrono::NaiveDate;
        Ok(Self {
            date: NaiveDate::parse_from_str(&c.date, "%Y-%m-%d")?,
            tokens: c.tokens,
            cost: c.cost,
            intensity: c.intensity,
        })
    }
}

impl From<&GraphData> for CachedGraphData {
    fn from(g: &GraphData) -> Self {
        Self {
            weeks: g
                .weeks
                .iter()
                .map(|week| {
                    week.iter()
                        .map(|day| day.as_ref().map(|d| d.into()))
                        .collect()
                })
                .collect(),
        }
    }
}

impl TryFrom<CachedGraphData> for GraphData {
    type Error = chrono::ParseError;

    fn try_from(g: CachedGraphData) -> Result<Self, Self::Error> {
        let weeks: Result<Vec<Vec<Option<ContributionDay>>>, _> = g
            .weeks
            .into_iter()
            .map(|week| {
                week.into_iter()
                    .map(|day| day.map(|d| d.try_into()).transpose())
                    .collect()
            })
            .collect();
        Ok(Self { weeks: weeks? })
    }
}

impl From<&UsageData> for CachedUsageData {
    fn from(u: &UsageData) -> Self {
        Self {
            models: u.models.iter().map(|m| m.into()).collect(),
            agents: u.agents.iter().map(|a| a.into()).collect(),
            daily: u.daily.iter().map(|d| d.into()).collect(),
            hourly: u.hourly.iter().map(|h| h.into()).collect(),
            prices: u.prices_daily.iter().map(|p| p.into()).collect(),
            thinking: u.thinking_daily.iter().map(|t| t.into()).collect(),
            speeds: u.speeds_daily.iter().map(|s| s.into()).collect(),
            codex_accounts: u.codex_accounts.iter().map(|a| a.into()).collect(),
            quota_value: (&u.quota_value).into(),
            graph: u.graph.as_ref().map(|g| g.into()),
            total_tokens: u.total_tokens,
            total_cost: u.total_cost,
            current_streak: u.current_streak,
            longest_streak: u.longest_streak,
        }
    }
}

impl TryFrom<CachedUsageData> for UsageData {
    type Error = chrono::ParseError;

    fn try_from(u: CachedUsageData) -> Result<Self, Self::Error> {
        let daily: Result<Vec<DailyUsage>, _> = u.daily.into_iter().map(|d| d.try_into()).collect();
        let hourly: Result<Vec<HourlyUsage>, _> =
            u.hourly.into_iter().map(|h| h.try_into()).collect();
        let prices_daily: Result<Vec<PriceUsage>, _> =
            u.prices.into_iter().map(|p| p.try_into()).collect();
        let thinking_daily: Result<Vec<ThinkingUsage>, _> =
            u.thinking.into_iter().map(|t| t.try_into()).collect();
        let speeds_daily: Result<Vec<SpeedUsage>, _> =
            u.speeds.into_iter().map(|s| s.try_into()).collect();
        let codex_accounts: Result<Vec<CodexAccountUsage>, _> =
            u.codex_accounts.into_iter().map(|a| a.try_into()).collect();
        let graph: Option<Result<GraphData, _>> = u.graph.map(|g| g.try_into());
        let prices = build_cached_price_summaries(&prices_daily.clone()?);
        let thinking = build_cached_thinking_summaries(&thinking_daily.clone()?);
        let speeds = build_cached_speed_summaries(&speeds_daily.clone()?);

        Ok(Self {
            models: u.models.into_iter().map(|m| m.into()).collect(),
            agents: normalize_cached_agents(u.agents),
            daily: daily?,
            hourly: hourly?,
            prices,
            prices_daily: prices_daily?,
            thinking,
            thinking_daily: thinking_daily?,
            speeds,
            speeds_daily: speeds_daily?,
            codex_accounts: codex_accounts?,
            quota_value: u.quota_value.try_into()?,
            graph: graph.transpose()?,
            total_tokens: u.total_tokens,
            total_cost: u.total_cost,
            loading: false,
            error: None,
            current_streak: u.current_streak,
            longest_streak: u.longest_streak,
        })
    }
}

fn normalize_cached_agents(agents: Vec<CachedAgentUsage>) -> Vec<AgentUsage> {
    let mut merged: BTreeMap<String, AgentUsage> = BTreeMap::new();
    let mut clients_by_agent: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for cached in agents {
        let normalized_agent = normalize_cached_agent_name(&cached.agent, &cached.clients);
        let entry = merged
            .entry(normalized_agent.clone())
            .or_insert_with(|| AgentUsage {
                agent: normalized_agent.clone(),
                clients: String::new(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                message_count: 0,
            });

        let tokens: TokenBreakdown = cached.tokens.into();
        entry.tokens.input = entry.tokens.input.saturating_add(tokens.input);
        entry.tokens.output = entry.tokens.output.saturating_add(tokens.output);
        entry.tokens.cache_read = entry.tokens.cache_read.saturating_add(tokens.cache_read);
        entry.tokens.cache_write = entry.tokens.cache_write.saturating_add(tokens.cache_write);
        entry.tokens.reasoning = entry.tokens.reasoning.saturating_add(tokens.reasoning);
        entry.cost += cached.cost;
        entry.message_count = entry.message_count.saturating_add(cached.message_count);

        let client_set = clients_by_agent.entry(normalized_agent).or_default();
        for client in cached
            .clients
            .split(", ")
            .filter(|client| !client.is_empty())
        {
            client_set.insert(client.to_string());
        }
    }

    let mut agents = merged.into_values().collect::<Vec<_>>();
    for agent in &mut agents {
        if let Some(clients) = clients_by_agent.get(&agent.agent) {
            agent.clients = clients.iter().cloned().collect::<Vec<_>>().join(", ");
        }
    }
    agents
}

fn normalize_cached_agent_name(agent: &str, clients: &str) -> String {
    if clients.split(", ").any(|client| client == "opencode") {
        sessions::normalize_opencode_agent_name(agent)
    } else {
        sessions::normalize_agent_name(agent)
    }
}

fn build_cached_price_summaries(prices_daily: &[PriceUsage]) -> Vec<PriceSummary> {
    let mut summary_map: BTreeMap<String, PriceSummary> = BTreeMap::new();

    for row in prices_daily {
        let entry = summary_map
            .entry(row.model.clone())
            .or_insert_with(|| PriceSummary {
                model: row.model.clone(),
                provider: row.provider.clone(),
                pricing_source: row.pricing_source.clone(),
                matched_key: row.matched_key.clone(),
                latest_date: row.date,
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                clients: Default::default(),
                message_count: 0,
                input_price_per_million: row.input_price_per_million,
                output_price_per_million: row.output_price_per_million,
                cache_read_price_per_million: row.cache_read_price_per_million,
                cache_write_price_per_million: row.cache_write_price_per_million,
            });

        if row.date > entry.latest_date {
            entry.latest_date = row.date;
            entry.input_price_per_million = row.input_price_per_million;
            entry.output_price_per_million = row.output_price_per_million;
            entry.cache_read_price_per_million = row.cache_read_price_per_million;
            entry.cache_write_price_per_million = row.cache_write_price_per_million;
        }

        if !entry
            .provider
            .split(", ")
            .any(|provider| provider == row.provider)
        {
            entry.provider = format!("{}, {}", entry.provider, row.provider);
        }
        match (
            entry.pricing_source.as_deref(),
            row.pricing_source.as_deref(),
        ) {
            (None, Some(source)) => entry.pricing_source = Some(source.to_string()),
            (Some(existing), Some(source)) if existing != source && existing != "Mixed" => {
                entry.pricing_source = Some("Mixed".to_string())
            }
            _ => {}
        }

        entry.tokens.input = entry.tokens.input.saturating_add(row.tokens.input);
        entry.tokens.output = entry.tokens.output.saturating_add(row.tokens.output);
        entry.tokens.cache_read = entry
            .tokens
            .cache_read
            .saturating_add(row.tokens.cache_read);
        entry.tokens.cache_write = entry
            .tokens
            .cache_write
            .saturating_add(row.tokens.cache_write);
        entry.tokens.reasoning = entry.tokens.reasoning.saturating_add(row.tokens.reasoning);
        entry.cost += row.cost;
        entry.clients.extend(row.clients.iter().cloned());
        entry.message_count = entry.message_count.saturating_add(row.message_count);
    }

    summary_map.into_values().collect()
}

fn build_cached_thinking_summaries(thinking_daily: &[ThinkingUsage]) -> Vec<ThinkingSummary> {
    let mut summary_map: BTreeMap<String, ThinkingSummary> = BTreeMap::new();
    let mut latest_dates: BTreeMap<String, chrono::NaiveDate> = BTreeMap::new();

    for row in thinking_daily {
        let entry = summary_map
            .entry(row.model.clone())
            .or_insert_with(|| ThinkingSummary {
                model: row.model.clone(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                clients: Default::default(),
                message_count: 0,
                thirty_day_trend_pct: None,
            });
        entry.tokens.input = entry.tokens.input.saturating_add(row.tokens.input);
        entry.tokens.output = entry.tokens.output.saturating_add(row.tokens.output);
        entry.tokens.cache_read = entry
            .tokens
            .cache_read
            .saturating_add(row.tokens.cache_read);
        entry.tokens.cache_write = entry
            .tokens
            .cache_write
            .saturating_add(row.tokens.cache_write);
        entry.tokens.reasoning = entry.tokens.reasoning.saturating_add(row.tokens.reasoning);
        entry.cost += row.cost;
        entry.clients.extend(row.clients.iter().cloned());
        entry.message_count = entry.message_count.saturating_add(row.message_count);

        latest_dates
            .entry(row.model.clone())
            .and_modify(|date| {
                if row.date > *date {
                    *date = row.date;
                }
            })
            .or_insert(row.date);
    }

    for summary in summary_map.values_mut() {
        let Some(latest_date) = latest_dates.get(&summary.model).copied() else {
            continue;
        };
        let recent_start = latest_date - chrono::Duration::days(29);
        let previous_start = recent_start - chrono::Duration::days(30);
        let mut recent_output = 0_u64;
        let mut recent_reasoning = 0_u64;
        let mut previous_output = 0_u64;
        let mut previous_reasoning = 0_u64;

        for row in thinking_daily
            .iter()
            .filter(|row| row.model == summary.model)
        {
            if row.date >= recent_start && row.date <= latest_date {
                recent_output = recent_output.saturating_add(row.tokens.output);
                recent_reasoning = recent_reasoning.saturating_add(row.tokens.reasoning);
            } else if row.date >= previous_start && row.date < recent_start {
                previous_output = previous_output.saturating_add(row.tokens.output);
                previous_reasoning = previous_reasoning.saturating_add(row.tokens.reasoning);
            }
        }

        let recent_generated = recent_output.saturating_add(recent_reasoning);
        let previous_generated = previous_output.saturating_add(previous_reasoning);
        summary.thirty_day_trend_pct = if recent_generated == 0 && previous_generated == 0 {
            Some(0.0)
        } else if previous_generated == 0 || previous_reasoning == 0 {
            None
        } else {
            let recent_rate = recent_reasoning as f64 / recent_generated as f64;
            let previous_rate = previous_reasoning as f64 / previous_generated as f64;
            if previous_rate <= f64::EPSILON {
                None
            } else {
                Some(((recent_rate - previous_rate) / previous_rate) * 100.0)
            }
        };
    }

    summary_map.into_values().collect()
}

fn build_cached_speed_summaries(speeds_daily: &[SpeedUsage]) -> Vec<SpeedSummary> {
    let mut summary_map: BTreeMap<(String, String, String), SpeedSummary> = BTreeMap::new();

    for row in speeds_daily {
        let key = (
            row.provider.clone(),
            row.model.clone(),
            row.thinking_level.clone(),
        );
        let entry = summary_map.entry(key).or_insert_with(|| SpeedSummary {
            model: row.model.clone(),
            provider: row.provider.clone(),
            thinking_level: row.thinking_level.clone(),
            latest_date: row.date,
            generated_tokens: 0,
            generation_duration_ms: 0,
            clients: Default::default(),
            sample_count: 0,
        });
        if row.date > entry.latest_date {
            entry.latest_date = row.date;
        }
        entry.generated_tokens = entry.generated_tokens.saturating_add(row.generated_tokens);
        entry.generation_duration_ms = entry
            .generation_duration_ms
            .saturating_add(row.generation_duration_ms);
        entry.clients.extend(row.clients.iter().cloned());
        entry.sample_count = entry.sample_count.saturating_add(row.sample_count);
    }

    let mut summaries: Vec<SpeedSummary> = summary_map.into_values().collect();
    summaries.sort_by(|a, b| {
        b.tokens_per_second()
            .total_cmp(&a.tokens_per_second())
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.thinking_level.cmp(&b.thinking_level))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    summaries
}

/// Result of loading the TUI cache — combines staleness check with data loading
/// to avoid double file I/O (previously is_cache_stale + load_cached_data both parsed the file).
pub enum CacheResult {
    /// Cache exists, is fresh (within TTL), and clients match exactly
    Fresh(UsageData),
    /// Cache exists and clients match (exact or subset), but needs background refresh
    Stale(UsageData),
    /// Cache missing, unreadable, unparseable, or clients don't match
    Miss,
}

/// How the cached client set relates to the currently enabled client set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientMatch {
    /// Cached clients are exactly the currently enabled clients
    Exact,
    /// Cached clients are a strict subset of the currently enabled clients.
    /// The cached data is still valid — it just doesn't cover the new clients yet.
    Subset,
    /// No usable overlap (superset, disjoint, or synthetic flag mismatch)
    Mismatch,
}
/// Load cached TUI data from disk with a single read/parse.
/// Returns Fresh/Stale/Miss so the caller can decide whether to
/// display cached data immediately and/or trigger a background refresh.
///
/// `enabled_clients` is the unified `HashSet<ClientFilter>` (Synthetic
/// included as a set member). The on-disk format keeps the legacy
/// `(enabled_clients: Vec<String>, include_synthetic: bool)` shape so
/// existing user caches keep working across upgrades — projection
/// happens here.
#[cfg_attr(not(test), allow(dead_code))]
pub fn load_cache(enabled_clients: &HashSet<ClientFilter>, group_by: &GroupBy) -> CacheResult {
    load_cache_with_filters(enabled_clients, group_by, None, None, None)
}

pub fn load_cache_with_filters(
    enabled_clients: &HashSet<ClientFilter>,
    group_by: &GroupBy,
    since: Option<&str>,
    until: Option<&str>,
    year: Option<&str>,
) -> CacheResult {
    let Some(cache_path) = cache_file() else {
        return CacheResult::Miss;
    };
    let cached: Option<CachedTUIData> = match File::open(&cache_path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            serde_json::from_reader(reader).ok()
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            legacy_cache_files().into_iter().find_map(|path| {
                let file = File::open(path).ok()?;
                let reader = BufReader::new(file);
                serde_json::from_reader(reader).ok()
            })
        }
        Err(_) => None,
    };
    let Some(cached) = cached else {
        return CacheResult::Miss;
    };
    if cached.schema_version > CACHE_SCHEMA_VERSION {
        return CacheResult::Miss;
    }
    let schema_outdated = cached.schema_version < CACHE_SCHEMA_VERSION;
    let cached_group_by = cached
        .group_by
        .as_deref()
        .and_then(|value: &str| value.parse::<GroupBy>().ok());
    if schema_outdated && cached_group_by.is_none() {
        return CacheResult::Miss;
    }

    if cached_group_by.as_ref() != Some(group_by) {
        return CacheResult::Miss;
    }
    if cached.since.as_deref() != since
        || cached.until.as_deref() != until
        || cached.year.as_deref() != year
    {
        return CacheResult::Miss;
    }

    // Check how cached clients relate to enabled clients
    let client_match = check_client_match(
        enabled_clients,
        &cached.enabled_clients,
        cached.include_synthetic,
    );

    if client_match == ClientMatch::Mismatch {
        return CacheResult::Miss;
    }
    // Convert cached data to UsageData
    let data = match cached.data.try_into() {
        Ok(d) => d,
        Err(_) => return CacheResult::Miss,
    };

    if schema_outdated || client_match == ClientMatch::Subset {
        return CacheResult::Stale(data);
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let cache_age = now.saturating_sub(cached.timestamp);
    if cache_age > CACHE_STALE_THRESHOLD_MS {
        CacheResult::Stale(data)
    } else {
        CacheResult::Fresh(data)
    }
}

/// Determine how the cached client set relates to the currently enabled set.
///
/// - `Exact`    — same clients, same synthetic flag
/// - `Subset`   — cached clients ⊆ enabled clients (e.g. update added a new client),
///   and cached doesn't carry data the user doesn't want
/// - `Mismatch` — anything else (superset, disjoint, unwanted synthetic data)
///
/// Cached side stays in the legacy `(Vec<String>, bool)` shape so we can
/// read pre-refactor cache files without a migration step. Enabled side
/// is the new unified `HashSet<ClientFilter>`.
fn check_client_match(
    enabled_clients: &HashSet<ClientFilter>,
    cached_clients: &[String],
    cached_include_synthetic: bool,
) -> ClientMatch {
    let include_synthetic = enabled_clients.contains(&ClientFilter::Synthetic);

    // If cache has synthetic data but user doesn't want it → mismatch
    // (showing unwanted data is worse than a cache miss)
    if cached_include_synthetic && !include_synthetic {
        return ClientMatch::Mismatch;
    }

    // Every cached client must exist in the enabled set. Compare on the
    // canonical lowercase id so we don't have to round-trip through
    // ClientId for clients that map 1:1.
    for cached_client_str in cached_clients {
        let in_enabled = enabled_clients
            .iter()
            .any(|f| f.as_filter_str() == cached_client_str);
        if !in_enabled {
            return ClientMatch::Mismatch;
        }
    }

    // Exact match requires same set membership on BOTH sides:
    //   |enabled non-synthetic| == |cached_clients|  AND
    //   include_synthetic == cached_include_synthetic
    let enabled_non_synthetic = enabled_clients.len() - usize::from(include_synthetic);
    let same_size = enabled_non_synthetic == cached_clients.len();
    let same_synthetic = include_synthetic == cached_include_synthetic;

    if same_size && same_synthetic {
        ClientMatch::Exact
    } else {
        ClientMatch::Subset
    }
}

/// Save TUI data to disk cache.
///
/// On-disk schema keeps the legacy `(Vec<String> enabled_clients, bool
/// include_synthetic)` pair so caches written by older releases remain
/// readable across upgrades. We project the unified
/// `HashSet<ClientFilter>` here.
pub fn save_cached_data(
    data: &UsageData,
    enabled_clients: &HashSet<ClientFilter>,
    group_by: &GroupBy,
) {
    save_cached_data_with_filters(data, enabled_clients, group_by, None, None, None)
}

pub fn save_cached_data_with_filters(
    data: &UsageData,
    enabled_clients: &HashSet<ClientFilter>,
    group_by: &GroupBy,
    since: Option<&str>,
    until: Option<&str>,
    year: Option<&str>,
) {
    let Some(cache_path) = cache_file() else {
        return;
    };

    // Ensure cache directory exists
    if let Some(dir) = cache_path.parent() {
        if fs::create_dir_all(dir).is_err() {
            return;
        }
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Project unified set into the legacy on-disk shape.
    let include_synthetic = enabled_clients.contains(&ClientFilter::Synthetic);
    let mut clients_vec: Vec<String> = enabled_clients
        .iter()
        .filter(|f| !matches!(f, ClientFilter::Synthetic))
        .map(|f| f.as_filter_str().to_string())
        .collect();
    // Sort so the cache key is deterministic across runs / HashSet
    // iteration order — otherwise unrelated runs would invalidate each
    // other's caches just because the JSON ordering shuffled.
    clients_vec.sort();

    let cached = CachedTUIData {
        schema_version: CACHE_SCHEMA_VERSION,
        timestamp,
        enabled_clients: clients_vec,
        include_synthetic,
        group_by: Some(group_by.to_string()),
        since: since.map(ToOwned::to_owned),
        until: until.map(ToOwned::to_owned),
        year: year.map(ToOwned::to_owned),
        data: data.into(),
    };

    // INVARIANT: All cache writes use atomic temp-file rename. NEVER delete
    // the canonical cache file before writing — a partial save or process
    // crash between delete and rename would lose the cache. The temp-file
    // pattern makes corruption-on-crash impossible.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let temp_path = cache_path.with_file_name(format!(
        ".{}.{}.{:x}.tmp",
        cache_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("tui-data-cache.json"),
        std::process::id(),
        nanos
    ));
    let file = match File::create(&temp_path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let writer = BufWriter::new(file);

    if serde_json::to_writer(writer, &cached).is_ok() {
        let _ = tokscale_core::fs_atomic::replace_file(&temp_path, &cache_path);
    } else {
        let _ = fs::remove_file(&temp_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{env, fs};
    use tempfile::TempDir;

    /// Build a unified filter set. Pass `synthetic=true` to include
    /// `ClientFilter::Synthetic` as a set member (the new way to express
    /// "user has synthetic enabled").
    fn make_filters(filters: &[ClientFilter], synthetic: bool) -> HashSet<ClientFilter> {
        let mut set: HashSet<ClientFilter> = filters.iter().copied().collect();
        if synthetic {
            set.insert(ClientFilter::Synthetic);
        }
        set
    }

    fn cached_agent(agent: &str, clients: &str, total_seed: u64) -> CachedAgentUsage {
        CachedAgentUsage {
            agent: agent.to_string(),
            clients: clients.to_string(),
            tokens: CachedTokenBreakdown {
                input: total_seed,
                output: 1,
                cache_read: 2,
                cache_write: 3,
                reasoning: 4,
            },
            cost: total_seed as f64,
            message_count: 1,
        }
    }

    #[test]
    fn test_normalize_cached_agents_merges_opencode_display_variants() {
        let agents = normalize_cached_agents(vec![
            cached_agent("Sisyphus", "opencode", 10),
            cached_agent("\u{200B} Sisyphus   -   Ultraworker", "opencode", 20),
            cached_agent(
                "\u{200B}\u{200B}\u{200B} Prometheus    Plan Builder",
                "opencode",
                30,
            ),
        ]);

        assert_eq!(agents.len(), 2);
        let sisyphus = agents
            .iter()
            .find(|agent| agent.agent == "Sisyphus")
            .unwrap();
        assert_eq!(sisyphus.clients, "opencode");
        assert_eq!(sisyphus.message_count, 2);
        assert_eq!(sisyphus.tokens.input, 30);
        assert!((sisyphus.cost - 30.0).abs() < f64::EPSILON);

        let prometheus = agents
            .iter()
            .find(|agent| agent.agent == "Prometheus")
            .unwrap();
        assert_eq!(prometheus.message_count, 1);
    }

    // ── check_client_match ──────────────────────────────────────────

    #[test]
    fn test_exact_match() {
        let enabled = make_filters(&[ClientFilter::Claude, ClientFilter::Opencode], false);
        let cached = vec!["claude".to_string(), "opencode".to_string()];
        assert_eq!(
            check_client_match(&enabled, &cached, false),
            ClientMatch::Exact,
        );
    }

    #[test]
    fn test_subset_new_client_added() {
        // Simulates: update added Qwen, cache only has Claude + OpenCode
        let enabled = make_filters(
            &[
                ClientFilter::Claude,
                ClientFilter::Opencode,
                ClientFilter::Qwen,
            ],
            false,
        );
        let cached = vec!["claude".to_string(), "opencode".to_string()];
        assert_eq!(
            check_client_match(&enabled, &cached, false),
            ClientMatch::Subset,
        );
    }

    #[test]
    fn test_subset_synthetic_added() {
        // Cache was saved without synthetic, now user enables it
        let enabled = make_filters(&[ClientFilter::Claude], true);
        let cached = vec!["claude".to_string()];
        assert_eq!(
            check_client_match(&enabled, &cached, false),
            ClientMatch::Subset,
        );
    }

    #[test]
    fn test_mismatch_superset() {
        // Cache has more clients than enabled (user narrowed filter)
        let enabled = make_filters(&[ClientFilter::Claude], false);
        let cached = vec!["claude".to_string(), "opencode".to_string()];
        assert_eq!(
            check_client_match(&enabled, &cached, false),
            ClientMatch::Mismatch,
        );
    }

    #[test]
    fn test_mismatch_disjoint() {
        let enabled = make_filters(&[ClientFilter::Claude], false);
        let cached = vec!["opencode".to_string()];
        assert_eq!(
            check_client_match(&enabled, &cached, false),
            ClientMatch::Mismatch,
        );
    }

    #[test]
    fn test_mismatch_unwanted_synthetic() {
        // Cache has synthetic data but user doesn't want it
        let enabled = make_filters(&[ClientFilter::Claude], false);
        let cached = vec!["claude".to_string()];
        assert_eq!(
            check_client_match(&enabled, &cached, true),
            ClientMatch::Mismatch,
        );
    }

    #[test]
    fn test_exact_with_synthetic() {
        let enabled = make_filters(&[ClientFilter::Claude], true);
        let cached = vec!["claude".to_string()];
        assert_eq!(
            check_client_match(&enabled, &cached, true),
            ClientMatch::Exact,
        );
    }

    #[test]
    fn test_subset_both_new_client_and_synthetic() {
        // Update added new client AND user also enabled synthetic
        let enabled = make_filters(&[ClientFilter::Claude, ClientFilter::Qwen], true);
        let cached = vec!["claude".to_string()];
        assert_eq!(
            check_client_match(&enabled, &cached, false),
            ClientMatch::Subset,
        );
    }

    #[test]
    fn test_empty_cache_is_subset() {
        let enabled = make_filters(&[ClientFilter::Claude], false);
        let cached: Vec<String> = vec![];
        assert_eq!(
            check_client_match(&enabled, &cached, false),
            ClientMatch::Subset,
        );
    }

    #[test]
    #[serial]
    fn test_load_cache_misses_for_legacy_schema_without_group_by() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "includeSynthetic": false,
  "data": {
    "models": [],
    "daily": [],
    "graph": null,
    "totalTokens": 0,
    "totalCost": 0.0,
    "currentStreak": 0,
    "longestStreak": 0
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        assert!(matches!(
            load_cache(&clients, &GroupBy::Model),
            CacheResult::Miss
        ));

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn test_load_cache_misses_when_group_by_differs() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "schemaVersion": 4,
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "includeSynthetic": false,
  "groupBy": "model",
  "data": {
    "models": [],
    "daily": [],
    "graph": null,
    "totalTokens": 0,
    "totalCost": 0.0,
    "currentStreak": 0,
    "longestStreak": 0
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        assert!(matches!(
            load_cache(&clients, &GroupBy::WorkspaceModel),
            CacheResult::Miss
        ));

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn test_load_cache_stale_legacy_daily_models_without_display_fields() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "schemaVersion": 3,
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "includeSynthetic": false,
  "groupBy": "model",
  "data": {
    "models": [],
    "agents": [],
    "daily": [{
      "date": "2026-03-18",
      "tokens": {
        "input": 10,
        "output": 5,
        "cacheRead": 0,
        "cacheWrite": 0,
        "reasoning": 0
      },
      "cost": 1.25,
      "models": [[
        "claude-sonnet-4-5",
        {
          "client": "claude",
          "tokens": {
            "input": 10,
            "output": 5,
            "cacheRead": 0,
            "cacheWrite": 0,
            "reasoning": 0
          },
          "cost": 1.25
        }
      ]]
    }],
    "graph": null,
    "totalTokens": 15,
    "totalCost": 1.25,
    "currentStreak": 1,
    "longestStreak": 1
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        match load_cache(&clients, &GroupBy::Model) {
            CacheResult::Stale(data) => {
                let source = data.daily[0].source_breakdown.get("claude").unwrap();
                let daily_model = source.models.get("claude-sonnet-4-5").unwrap();
                assert_eq!(daily_model.display_name, "claude-sonnet-4-5");
                assert_eq!(daily_model.color_key, "claude-sonnet-4-5");
            }
            other => panic!(
                "expected stale legacy cache, got {:?}",
                other_variant_name(&other)
            ),
        }

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn test_load_cache_reads_source_breakdown_from_current_schema() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "schemaVersion": 17,
  "timestamp": 9999999999999,
  "enabledClients": ["claude", "cursor"],
  "includeSynthetic": false,
  "groupBy": "model",
  "data": {
    "models": [],
    "agents": [],
    "daily": [{
      "date": "2026-03-18",
      "tokens": {
        "input": 30,
        "output": 15,
        "cacheRead": 0,
        "cacheWrite": 0,
        "reasoning": 0
      },
      "cost": 3.25,
      "sourceBreakdown": [[
        "claude",
        {
          "tokens": {
            "input": 10,
            "output": 5,
            "cacheRead": 0,
            "cacheWrite": 0,
            "reasoning": 0
          },
          "cost": 1.25,
          "models": [[
            "claude-sonnet-4-5",
            {
              "provider": "anthropic",
              "displayName": "claude-sonnet-4-5",
              "colorKey": "claude-sonnet-4-5",
              "tokens": {
                "input": 10,
                "output": 5,
                "cacheRead": 0,
                "cacheWrite": 0,
                "reasoning": 0
              },
              "cost": 1.25
            }
          ]]
        }
      ], [
        "cursor",
        {
          "tokens": {
            "input": 20,
            "output": 10,
            "cacheRead": 0,
            "cacheWrite": 0,
            "reasoning": 0
          },
          "cost": 2.0,
          "models": [[
            "claude-sonnet-4-5",
            {
              "provider": "anthropic",
              "displayName": "claude-sonnet-4-5",
              "colorKey": "claude-sonnet-4-5",
              "tokens": {
                "input": 20,
                "output": 10,
                "cacheRead": 0,
                "cacheWrite": 0,
                "reasoning": 0
              },
              "cost": 2.0
            }
          ]]
        }
      ]]
    }],
    "graph": null,
    "totalTokens": 45,
    "totalCost": 3.25,
    "currentStreak": 1,
    "longestStreak": 1
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude, ClientFilter::Cursor], false);
        match load_cache(&clients, &GroupBy::Model) {
            CacheResult::Fresh(data) => {
                assert_eq!(data.daily[0].source_breakdown.len(), 2);
                let cursor = data.daily[0].source_breakdown.get("cursor").unwrap();
                let model = cursor.models.get("claude-sonnet-4-5").unwrap();
                assert_eq!(model.provider, "anthropic");
                assert_eq!(model.tokens.total(), 30);
            }
            other => panic!(
                "expected fresh current-schema cache, got {:?}",
                other_variant_name(&other)
            ),
        }

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn test_load_cache_round_trips_quota_value_from_current_schema() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let mut models = std::collections::BTreeSet::new();
        models.insert("gpt-5.4".to_string());
        let mut data = UsageData::default();
        data.quota_value = QuotaValueData {
            sample_count: 2,
            intervals: vec![QuotaValueInterval {
                account_hash: "acct_hash".to_string(),
                model: "gpt-5.4".to_string(),
                window_kind: "secondary".to_string(),
                end: chrono::NaiveDate::from_ymd_opt(2026, 1, 1)
                    .unwrap()
                    .and_hms_opt(1, 0, 0)
                    .unwrap(),
                quota_burn_pct: 4.0,
                api_value_usd: 8.0,
                subscription_cost_burned: 1.0,
                factor: Some(8.0),
                dollars_per_percent: Some(2.0),
                tokens: TokenBreakdown {
                    input: 10,
                    output: 20,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 5,
                },
                models,
                sample_count: 2,
                confidence: QuotaConfidence::Medium,
            }],
            points: vec![QuotaValuePoint {
                date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                model: "gpt-5.4".to_string(),
                window_kind: "secondary".to_string(),
                quota_burn_pct: 4.0,
                api_value_usd: 8.0,
                subscription_cost_burned: 1.0,
                factor: Some(8.0),
                dollars_per_percent: Some(2.0),
                interval_count: 1,
                confidence: QuotaConfidence::Medium,
            }],
            model_summaries: vec![QuotaModelSummary {
                model: "gpt-5.4".to_string(),
                window_kind: "secondary".to_string(),
                latest_date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                quota_burn_pct: 4.0,
                api_value_usd: 8.0,
                subscription_cost_burned: 1.0,
                factor: Some(8.0),
                dollars_per_percent: Some(2.0),
                tokens: TokenBreakdown {
                    input: 10,
                    output: 20,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 5,
                },
                interval_count: 1,
                confidence: QuotaConfidence::Medium,
            }],
        };

        let clients = make_filters(&[ClientFilter::Codex], false);
        save_cached_data(&data, &clients, &GroupBy::Model);

        match load_cache(&clients, &GroupBy::Model) {
            CacheResult::Fresh(loaded) => {
                assert_eq!(loaded.quota_value.sample_count, 2);
                assert_eq!(loaded.quota_value.intervals.len(), 1);
                assert_eq!(loaded.quota_value.intervals[0].account_hash, "acct_hash");
                assert_eq!(loaded.quota_value.intervals[0].model, "gpt-5.4");
                assert_eq!(
                    loaded.quota_value.intervals[0].confidence,
                    QuotaConfidence::Medium
                );
                assert_eq!(loaded.quota_value.points.len(), 1);
                assert_eq!(loaded.quota_value.model_summaries.len(), 1);
                assert_eq!(loaded.quota_value.model_summaries[0].model, "gpt-5.4");
            }
            other => panic!(
                "expected fresh cache with quota value data, got {:?}",
                other_variant_name(&other)
            ),
        }

        match previous_home {
            Some(home) => unsafe {
                env::set_var("HOME", home);
            },
            None => unsafe {
                env::remove_var("HOME");
            },
        }
    }

    #[test]
    #[serial]
    fn test_cache_key_includes_date_filters() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let clients = make_filters(&[ClientFilter::Codex], false);
        let data = UsageData::default();
        save_cached_data_with_filters(
            &data,
            &clients,
            &GroupBy::Model,
            Some("2026-04-24"),
            None,
            None,
        );

        assert!(matches!(
            load_cache_with_filters(&clients, &GroupBy::Model, Some("2026-04-24"), None, None,),
            CacheResult::Fresh(_)
        ));
        assert!(matches!(
            load_cache_with_filters(&clients, &GroupBy::Model, Some("2026-04-25"), None, None,),
            CacheResult::Miss
        ));

        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn test_load_cache_stale_legacy_hourly_models_without_display_fields() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "schemaVersion": 5,
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "includeSynthetic": false,
  "groupBy": "model",
  "data": {
    "models": [],
    "agents": [],
    "daily": [],
    "hourly": [{
      "datetime": "2026-03-18 10:00:00",
      "tokens": {
        "input": 10,
        "output": 5,
        "cacheRead": 0,
        "cacheWrite": 0,
        "reasoning": 0
      },
      "cost": 1.25,
      "clients": ["claude"],
      "models": [[
        "claude-sonnet-4-5",
        {
          "tokens": {
            "input": 10,
            "output": 5,
            "cacheRead": 0,
            "cacheWrite": 0,
            "reasoning": 0
          },
          "cost": 1.25
        }
      ]],
      "messageCount": 1,
      "turnCount": 1
    }],
    "graph": null,
    "totalTokens": 15,
    "totalCost": 1.25,
    "currentStreak": 1,
    "longestStreak": 1
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        match load_cache(&clients, &GroupBy::Model) {
            CacheResult::Fresh(data) | CacheResult::Stale(data) => {
                let hourly_model = data.hourly[0].models.get("claude-sonnet-4-5").unwrap();
                assert_eq!(hourly_model.display_name, "claude-sonnet-4-5");
                assert_eq!(hourly_model.color_key, "claude-sonnet-4-5");
            }
            other => panic!("expected cache data, got {:?}", other_variant_name(&other)),
        }

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn test_load_cache_legacy_empty_client_falls_back_to_unknown() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            r#"{
  "schemaVersion": 3,
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "includeSynthetic": false,
  "groupBy": "model",
  "data": {
    "models": [],
    "agents": [],
    "daily": [{
      "date": "2026-03-18",
      "tokens": {
        "input": 10,
        "output": 5,
        "cacheRead": 0,
        "cacheWrite": 0,
        "reasoning": 0
      },
      "cost": 1.25,
      "models": [[
        "claude-sonnet-4-5",
        {
          "client": "",
          "tokens": {
            "input": 10,
            "output": 5,
            "cacheRead": 0,
            "cacheWrite": 0,
            "reasoning": 0
          },
          "cost": 1.25
        }
      ]]
    }],
    "graph": null,
    "totalTokens": 15,
    "totalCost": 1.25,
    "currentStreak": 1,
    "longestStreak": 1
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        match load_cache(&clients, &GroupBy::Model) {
            CacheResult::Stale(data) => {
                assert!(
                    data.daily[0].source_breakdown.contains_key("unknown"),
                    "empty client should fall back to 'unknown'"
                );
                let unknown = data.daily[0].source_breakdown.get("unknown").unwrap();
                assert_eq!(unknown.models.len(), 1);
                let model = unknown.models.get("claude-sonnet-4-5").unwrap();
                assert_eq!(model.tokens.total(), 15);
            }
            other => panic!(
                "expected stale legacy cache, got {:?}",
                other_variant_name(&other)
            ),
        }

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn load_cache_falls_back_to_legacy_dot_cache_path() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let previous_override = env::var_os("TOKSCALE_CONFIG_DIR");
        let previous_xdg_config_home = env::var_os("XDG_CONFIG_HOME");
        unsafe {
            env::set_var("HOME", temp_dir.path());
            env::remove_var("TOKSCALE_CONFIG_DIR");
            env::set_var("XDG_CONFIG_HOME", temp_dir.path().join(".xdg-config"));
        }

        let legacy_path = temp_dir.path().join(".cache/tokscale/tui-data-cache.json");
        fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        fs::write(
            &legacy_path,
            r#"{
  "schemaVersion": 17,
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "includeSynthetic": false,
  "groupBy": "model",
  "data": {
    "models": [],
    "agents": [],
    "daily": [],
    "hourly": [],
    "graph": null,
    "totalTokens": 0,
    "totalCost": 0.0,
    "currentStreak": 0,
    "longestStreak": 0
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        assert!(matches!(
            load_cache(&clients, &GroupBy::Model),
            CacheResult::Fresh(_)
        ));

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
        match previous_override {
            Some(value) => unsafe { env::set_var("TOKSCALE_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("TOKSCALE_CONFIG_DIR") },
        }
        match previous_xdg_config_home {
            Some(value) => unsafe { env::set_var("XDG_CONFIG_HOME", value) },
            None => unsafe { env::remove_var("XDG_CONFIG_HOME") },
        }
    }

    #[test]
    #[serial]
    fn load_cache_skips_legacy_when_overridden() {
        let temp_dir = TempDir::new().unwrap();
        let override_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let previous_override = env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            env::set_var("HOME", temp_dir.path());
            env::set_var("TOKSCALE_CONFIG_DIR", override_dir.path());
        }

        let legacy_path = temp_dir.path().join(".cache/tokscale/tui-data-cache.json");
        fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        fs::write(
            &legacy_path,
            r#"{
  "schemaVersion": 6,
  "timestamp": 9999999999999,
  "enabledClients": ["claude"],
  "includeSynthetic": false,
  "groupBy": "model",
  "data": {
    "models": [],
    "agents": [],
    "daily": [],
    "hourly": [],
    "graph": null,
    "totalTokens": 0,
    "totalCost": 0.0,
    "currentStreak": 0,
    "longestStreak": 0
  }
}"#,
        )
        .unwrap();

        let clients = make_filters(&[ClientFilter::Claude], false);
        assert!(matches!(
            load_cache(&clients, &GroupBy::Model),
            CacheResult::Miss
        ));

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
        match previous_override {
            Some(value) => unsafe { env::set_var("TOKSCALE_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("TOKSCALE_CONFIG_DIR") },
        }
    }

    #[test]
    #[serial]
    fn save_cached_data_does_not_delete_destination() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let previous_override = env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            env::set_var("HOME", temp_dir.path());
            env::remove_var("TOKSCALE_CONFIG_DIR");
        }

        let cache_path = cache_file().unwrap();
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let old_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        fs::write(
            &cache_path,
            format!(
                r#"{{
  "schemaVersion": 6,
  "timestamp": {old_timestamp},
  "enabledClients": ["claude"],
  "includeSynthetic": false,
  "groupBy": "model",
  "data": {{
    "models": [],
    "agents": [],
    "daily": [],
    "hourly": [],
    "graph": null,
    "totalTokens": 0,
    "totalCost": 0.0,
    "currentStreak": 0,
    "longestStreak": 0
  }}
}}"#
            ),
        )
        .unwrap();
        assert!(fs::metadata(&cache_path).is_ok());

        let clients = make_filters(&[ClientFilter::Claude], false);
        save_cached_data(&UsageData::default(), &clients, &GroupBy::Model);

        let metadata = fs::metadata(&cache_path).unwrap();
        assert!(metadata.is_file());
        let saved: CachedTUIData = serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
        assert!(saved.timestamp >= old_timestamp);

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
        match previous_override {
            Some(value) => unsafe { env::set_var("TOKSCALE_CONFIG_DIR", value) },
            None => unsafe { env::remove_var("TOKSCALE_CONFIG_DIR") },
        }
    }

    fn other_variant_name(result: &CacheResult) -> &'static str {
        match result {
            CacheResult::Fresh(_) => "Fresh",
            CacheResult::Stale(_) => "Stale",
            CacheResult::Miss => "Miss",
        }
    }
}
