use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, Timelike};
use tokio::runtime::{Handle, Runtime};

use tokscale_core::pricing::PricingService;
use tokscale_core::sessions::{CodexQuotaSample, UnifiedMessage};
use tokscale_core::{
    normalize_model_for_grouping, parse_local_usage_with_pricing, sessions, ClientId, GroupBy,
    LocalParseOptions,
};

/// Returns the scanner settings that `DataLoader` should use when building
/// `LocalParseOptions`. Under `#[cfg(test)]` this intentionally ignores
/// `~/.config/tokscale/settings.json` so data-loader unit tests stay
/// hermetic across developer machines; production builds still honor
/// user-configured paths.
#[cfg(not(test))]
fn data_loader_scanner_settings() -> tokscale_core::scanner::ScannerSettings {
    crate::tui::settings::load_scanner_settings()
}

#[cfg(test)]
fn data_loader_scanner_settings() -> tokscale_core::scanner::ScannerSettings {
    tokscale_core::scanner::ScannerSettings::default()
}

const MIN_REASONABLE_TOKENS_PER_SECOND: f64 = 30.0;
const MAX_REASONABLE_TOKENS_PER_SECOND: f64 = 1000.0;

#[derive(Debug, Clone, Default)]
pub struct TokenBreakdown {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
}

impl TokenBreakdown {
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
            .saturating_add(self.reasoning)
    }
}

#[derive(Debug, Clone)]
pub struct ModelUsage {
    pub model: String,
    pub provider: String,
    pub client: String,
    pub workspace_key: Option<String>,
    pub workspace_label: Option<String>,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub session_count: u32,
}

#[derive(Debug, Clone)]
pub struct AgentUsage {
    pub agent: String,
    pub clients: String,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub message_count: u32,
}

#[derive(Debug, Clone)]
pub struct DailyModelInfo {
    /// API provider identifier (e.g. "anthropic", "openai").
    ///
    /// **Caveat**: For `GroupBy::Model`, `GroupBy::ClientModel`, and
    /// `GroupBy::WorkspaceModel`, multiple providers may be merged into a
    /// single daily model entry.  In that case this field retains whichever
    /// provider was seen first and is **not** authoritative.  Only treat it
    /// as exact when `group_by == GroupBy::ClientProviderModel`.
    pub provider: String,
    pub display_name: String,
    pub color_key: String,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub messages: u64,
}

#[derive(Debug, Clone)]
pub struct DailySourceInfo {
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub models: BTreeMap<String, DailyModelInfo>,
}

#[derive(Debug, Clone)]
pub struct DailyUsage {
    pub date: NaiveDate,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub source_breakdown: BTreeMap<String, DailySourceInfo>,
    pub message_count: u32,
    pub turn_count: u32,
}

#[derive(Debug, Clone)]
pub struct HourlyModelInfo {
    pub provider: String,
    pub display_name: String,
    pub color_key: String,
    pub tokens: TokenBreakdown,
    pub cost: f64,
}

#[derive(Debug, Clone)]
pub struct HourlyUsage {
    pub datetime: NaiveDateTime,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub clients: BTreeSet<String>,
    pub models: BTreeMap<String, HourlyModelInfo>,
    pub message_count: u32,
    pub turn_count: u32,
}

#[derive(Debug, Clone)]
pub struct PriceUsage {
    pub date: NaiveDate,
    pub model: String,
    pub provider: String,
    pub pricing_source: Option<String>,
    pub matched_key: Option<String>,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub clients: BTreeSet<String>,
    pub message_count: u32,
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
    pub cache_read_price_per_million: Option<f64>,
    pub cache_write_price_per_million: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct PriceSummary {
    pub model: String,
    pub provider: String,
    pub pricing_source: Option<String>,
    pub matched_key: Option<String>,
    pub latest_date: NaiveDate,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub clients: BTreeSet<String>,
    pub message_count: u32,
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
    pub cache_read_price_per_million: Option<f64>,
    pub cache_write_price_per_million: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ThinkingUsage {
    pub date: NaiveDate,
    pub model: String,
    pub provider: String,
    pub thinking_level: String,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub clients: BTreeSet<String>,
    pub message_count: u32,
}

#[derive(Debug, Clone)]
pub struct ThinkingSummary {
    pub model: String,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub clients: BTreeSet<String>,
    pub message_count: u32,
    pub thirty_day_trend_pct: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct SpeedUsage {
    pub date: NaiveDate,
    pub model: String,
    pub provider: String,
    pub thinking_level: String,
    pub generated_tokens: u64,
    pub generation_duration_ms: u64,
    pub clients: BTreeSet<String>,
    pub sample_count: u32,
}

impl SpeedUsage {
    pub fn tokens_per_second(&self) -> f64 {
        if self.generation_duration_ms == 0 {
            return 0.0;
        }
        self.generated_tokens as f64 / (self.generation_duration_ms as f64 / 1000.0)
    }
}

#[derive(Debug, Clone)]
pub struct SpeedSummary {
    pub model: String,
    pub provider: String,
    pub thinking_level: String,
    pub latest_date: NaiveDate,
    pub generated_tokens: u64,
    pub generation_duration_ms: u64,
    pub clients: BTreeSet<String>,
    pub sample_count: u32,
}

impl SpeedSummary {
    pub fn tokens_per_second(&self) -> f64 {
        if self.generation_duration_ms == 0 {
            return 0.0;
        }
        self.generated_tokens as f64 / (self.generation_duration_ms as f64 / 1000.0)
    }
}

fn has_reasonable_tokens_per_second(generated_tokens: u64, duration_ms: u64) -> bool {
    if generated_tokens == 0 || duration_ms == 0 {
        return false;
    }

    let tokens_per_second = generated_tokens as f64 / (duration_ms as f64 / 1000.0);
    (MIN_REASONABLE_TOKENS_PER_SECOND..=MAX_REASONABLE_TOKENS_PER_SECOND)
        .contains(&tokens_per_second)
}

#[derive(Debug, Clone)]
pub struct CodexAccountUsage {
    pub account_hash: String,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub paid_cost: Option<f64>,
    pub active_month_count: Option<u32>,
    pub message_count: u32,
    pub turn_count: u32,
    pub session_count: u32,
    pub first_date: Option<NaiveDate>,
    pub latest_date: Option<NaiveDate>,
}

#[derive(Debug, Clone, Default)]
pub struct QuotaValueData {
    pub intervals: Vec<QuotaValueInterval>,
    pub points: Vec<QuotaValuePoint>,
    pub model_summaries: Vec<QuotaModelSummary>,
    pub sample_count: u32,
}

#[derive(Debug, Clone)]
pub struct QuotaValueInterval {
    pub account_hash: String,
    pub model: String,
    pub window_kind: String,
    pub end: NaiveDateTime,
    pub quota_burn_pct: f64,
    pub api_value_usd: f64,
    pub subscription_cost_burned: f64,
    pub factor: Option<f64>,
    pub dollars_per_percent: Option<f64>,
    pub tokens: TokenBreakdown,
    pub models: BTreeSet<String>,
    pub sample_count: u32,
    pub confidence: QuotaConfidence,
}

#[derive(Debug, Clone)]
pub struct QuotaValuePoint {
    pub date: NaiveDate,
    pub model: String,
    pub window_kind: String,
    pub quota_burn_pct: f64,
    pub api_value_usd: f64,
    pub subscription_cost_burned: f64,
    pub factor: Option<f64>,
    pub dollars_per_percent: Option<f64>,
    pub interval_count: u32,
    pub confidence: QuotaConfidence,
}

#[derive(Debug, Clone)]
pub struct QuotaModelSummary {
    pub model: String,
    pub window_kind: String,
    pub latest_date: NaiveDate,
    pub quota_burn_pct: f64,
    pub api_value_usd: f64,
    pub subscription_cost_burned: f64,
    pub factor: Option<f64>,
    pub dollars_per_percent: Option<f64>,
    pub tokens: TokenBreakdown,
    pub interval_count: u32,
    pub confidence: QuotaConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuotaConfidence {
    High,
    Medium,
    #[default]
    Low,
}

impl QuotaConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            QuotaConfidence::High => "High",
            QuotaConfidence::Medium => "Medium",
            QuotaConfidence::Low => "Low",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContributionDay {
    pub date: NaiveDate,
    pub tokens: u64,
    pub cost: f64,
    pub intensity: f64,
}

#[derive(Debug, Clone)]
pub struct GraphData {
    pub weeks: Vec<Vec<Option<ContributionDay>>>,
}

#[derive(Debug, Clone, Default)]
pub struct UsageData {
    pub models: Vec<ModelUsage>,
    pub agents: Vec<AgentUsage>,
    pub daily: Vec<DailyUsage>,
    pub hourly: Vec<HourlyUsage>,
    pub prices: Vec<PriceSummary>,
    pub prices_daily: Vec<PriceUsage>,
    pub thinking: Vec<ThinkingSummary>,
    pub thinking_daily: Vec<ThinkingUsage>,
    pub speeds: Vec<SpeedSummary>,
    pub speeds_daily: Vec<SpeedUsage>,
    pub codex_accounts: Vec<CodexAccountUsage>,
    pub quota_value: QuotaValueData,
    pub graph: Option<GraphData>,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub loading: bool,
    pub error: Option<String>,
    pub current_streak: u32,
    pub longest_streak: u32,
}

pub struct DataLoader {
    _sessions_path: Option<PathBuf>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub year: Option<String>,
}

const UNKNOWN_WORKSPACE_LABEL: &str = "Unknown workspace";
const UNKNOWN_WORKSPACE_GROUP_KEY: &str = "\0unknown-workspace";
const UNATTRIBUTED_CODEX_ACCOUNT: &str = "unattributed";
const CODEX_MONTHLY_SUBSCRIPTION_COST_USD: f64 = 200.0;

fn is_billable_codex_account_bucket(account_hash: &str) -> bool {
    !matches!(
        account_hash,
        UNATTRIBUTED_CODEX_ACCOUNT | "mixed" | "ledger_error"
    )
}

fn workspace_bucket(msg: &UnifiedMessage) -> (String, Option<String>, String) {
    match (&msg.workspace_key, &msg.workspace_label) {
        (Some(key), Some(label)) => (key.clone(), Some(key.clone()), label.clone()),
        (Some(key), None) => (
            key.clone(),
            Some(key.clone()),
            tokscale_core::sessions::workspace_label_from_key(key)
                .unwrap_or_else(|| UNKNOWN_WORKSPACE_LABEL.to_string()),
        ),
        _ => (
            UNKNOWN_WORKSPACE_GROUP_KEY.to_string(),
            None,
            UNKNOWN_WORKSPACE_LABEL.to_string(),
        ),
    }
}

fn workspace_model_display_label(workspace_label: &str, model: &str) -> String {
    format!("{workspace_label} / {model}")
}

fn workspace_model_daily_key(workspace_group_key: &str, model: &str) -> String {
    format!(
        "{}:{workspace_group_key}:{model}",
        workspace_group_key.len()
    )
}

fn daily_source_model_key(
    group_by: &GroupBy,
    workspace_group_key: &str,
    provider_id: &str,
    model: &str,
) -> String {
    match group_by {
        GroupBy::WorkspaceModel => workspace_model_daily_key(workspace_group_key, model),
        GroupBy::ClientProviderModel => format!("{provider_id}:{model}"),
        GroupBy::Model | GroupBy::ClientModel => model.to_string(),
    }
}

fn daily_source_model_display_name(
    group_by: &GroupBy,
    workspace_label: &str,
    provider_id: &str,
    model: &str,
) -> String {
    match group_by {
        GroupBy::WorkspaceModel => workspace_model_display_label(workspace_label, model),
        GroupBy::ClientProviderModel => format!("{provider_id} / {model}"),
        GroupBy::Model | GroupBy::ClientModel => model.to_string(),
    }
}

fn model_color_key(group_by: &GroupBy, _provider_id: &str, model: &str) -> String {
    match group_by {
        GroupBy::ClientProviderModel => model.to_string(),
        GroupBy::Model | GroupBy::ClientModel | GroupBy::WorkspaceModel => model.to_string(),
    }
}

fn hourly_model_key(group_by: &GroupBy, provider_id: &str, model: &str) -> String {
    match group_by {
        GroupBy::ClientProviderModel => format!("{provider_id}:{model}"),
        GroupBy::Model | GroupBy::ClientModel | GroupBy::WorkspaceModel => model.to_string(),
    }
}

fn hourly_model_display_name(group_by: &GroupBy, provider_id: &str, model: &str) -> String {
    match group_by {
        GroupBy::ClientProviderModel => format!("{provider_id} / {model}"),
        GroupBy::Model | GroupBy::ClientModel | GroupBy::WorkspaceModel => model.to_string(),
    }
}

impl DataLoader {
    pub fn new(sessions_path: Option<PathBuf>) -> Self {
        Self {
            _sessions_path: sessions_path,
            since: None,
            until: None,
            year: None,
        }
    }

    pub fn with_filters(
        sessions_path: Option<PathBuf>,
        since: Option<String>,
        until: Option<String>,
        year: Option<String>,
    ) -> Self {
        Self {
            _sessions_path: sessions_path,
            since,
            until,
            year,
        }
    }

    pub fn load(
        &self,
        enabled_clients: &[ClientId],
        group_by: &GroupBy,
        include_synthetic: bool,
    ) -> Result<UsageData> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .to_string_lossy()
            .to_string();

        let mut sources: Vec<String> = enabled_clients
            .iter()
            .map(|client| client.as_str().to_string())
            .collect();
        if include_synthetic {
            sources.push("synthetic".to_string());
        }

        let opts = LocalParseOptions {
            home_dir: Some(home),
            use_env_roots: true,
            clients: Some(sources),
            since: self.since.clone(),
            until: self.until.clone(),
            year: self.year.clone(),
            scanner_settings: data_loader_scanner_settings(),
        };

        let (usage, pricing) = if Handle::try_current().is_ok() {
            std::thread::scope(|s| {
                s.spawn(|| {
                    let rt = Runtime::new().map_err(|e| e.to_string())?;
                    let pricing = rt.block_on(load_pricing_for_display());
                    let usage =
                        rt.block_on(parse_local_usage_with_pricing(opts, pricing.as_deref()))?;
                    Ok((usage, pricing))
                })
                .join()
                .unwrap_or_else(|_| Err("data loader thread panicked".to_string()))
            })
        } else {
            let rt = Runtime::new()?;
            let pricing = rt.block_on(load_pricing_for_display());
            let usage = rt
                .block_on(parse_local_usage_with_pricing(opts, pricing.as_deref()))
                .map_err(anyhow::Error::msg)?;
            Ok((usage, pricing))
        }
        .map_err(anyhow::Error::msg)?;

        self.aggregate_messages_with_pricing(
            usage.messages,
            usage.quota_samples,
            group_by,
            pricing.as_deref(),
        )
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn load_with_pricing(
        &self,
        enabled_clients: &[ClientId],
        group_by: &GroupBy,
        include_synthetic: bool,
        pricing: &tokscale_core::pricing::PricingService,
    ) -> Result<UsageData> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .to_string_lossy()
            .to_string();

        let mut sources: Vec<String> = enabled_clients
            .iter()
            .map(|client| client.as_str().to_string())
            .collect();
        if include_synthetic {
            sources.push("synthetic".to_string());
        }

        let opts = LocalParseOptions {
            home_dir: Some(home),
            clients: Some(sources),
            since: self.since.clone(),
            until: self.until.clone(),
            year: self.year.clone(),
            use_env_roots: false,
            scanner_settings: data_loader_scanner_settings(),
        };

        let usage = if Handle::try_current().is_ok() {
            std::thread::scope(|s| {
                s.spawn(|| {
                    let rt = Runtime::new().map_err(|e| e.to_string())?;
                    rt.block_on(tokscale_core::parse_local_usage_with_pricing(
                        opts,
                        Some(pricing),
                    ))
                })
                .join()
                .unwrap_or_else(|_| Err("data loader thread panicked".to_string()))
            })
        } else {
            Runtime::new()?.block_on(tokscale_core::parse_local_usage_with_pricing(
                opts,
                Some(pricing),
            ))
        }
        .map_err(anyhow::Error::msg)?;

        self.aggregate_messages_with_pricing(
            usage.messages,
            usage.quota_samples,
            group_by,
            Some(pricing),
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn aggregate_messages(
        &self,
        messages: Vec<UnifiedMessage>,
        group_by: &GroupBy,
    ) -> Result<UsageData> {
        self.aggregate_messages_with_pricing(messages, Vec::new(), group_by, None)
    }

    fn aggregate_messages_with_pricing(
        &self,
        messages: Vec<UnifiedMessage>,
        quota_samples: Vec<CodexQuotaSample>,
        group_by: &GroupBy,
        pricing: Option<&PricingService>,
    ) -> Result<UsageData> {
        let mut model_map: HashMap<String, ModelUsage> = HashMap::new();
        let mut agent_map: HashMap<String, AgentUsage> = HashMap::new();
        let mut agent_clients: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut daily_map: HashMap<NaiveDate, DailyUsage> = HashMap::new();
        let mut hourly_map: HashMap<NaiveDateTime, HourlyUsage> = HashMap::new();
        let mut model_session_ids: HashMap<String, HashSet<String>> = HashMap::new();
        let prices_daily = build_price_rows(&messages, pricing);
        let prices = build_price_summaries(&prices_daily);
        let thinking_daily = build_thinking_rows(&messages);
        let thinking = build_thinking_summaries(&thinking_daily);
        let speeds_daily = build_speed_rows(&messages);
        let speeds = build_speed_summaries(&speeds_daily);
        let codex_accounts = build_codex_account_summaries(&messages);
        let quota_value = build_quota_value_data(&messages, &quota_samples);

        for msg in &messages {
            let normalized_model = normalize_model_for_grouping(&msg.model_id);
            let (workspace_group_key, workspace_key, workspace_label) = workspace_bucket(msg);
            let key = match group_by {
                GroupBy::Model => normalized_model.clone(),
                GroupBy::ClientModel => format!("{}:{}", msg.client, normalized_model),
                GroupBy::ClientProviderModel => {
                    format!("{}:{}:{}", msg.client, msg.provider_id, normalized_model)
                }
                GroupBy::WorkspaceModel => {
                    format!("{}:{}", workspace_group_key, normalized_model)
                }
            };
            let merge_clients = matches!(group_by, GroupBy::Model | GroupBy::WorkspaceModel);

            let model_entry = model_map.entry(key.clone()).or_insert_with(|| ModelUsage {
                model: normalized_model.clone(),
                provider: msg.provider_id.clone(),
                client: msg.client.clone(),
                workspace_key: if *group_by == GroupBy::WorkspaceModel {
                    workspace_key.clone()
                } else {
                    None
                },
                workspace_label: if *group_by == GroupBy::WorkspaceModel {
                    Some(workspace_label.clone())
                } else {
                    None
                },
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                session_count: 0,
            });

            if merge_clients && !model_entry.client.split(", ").any(|s| s == msg.client) {
                model_entry.client = format!("{}, {}", model_entry.client, msg.client);
            }

            if *group_by != GroupBy::ClientProviderModel
                && !model_entry
                    .provider
                    .split(", ")
                    .any(|p| p == msg.provider_id)
            {
                model_entry.provider = format!("{}, {}", model_entry.provider, msg.provider_id);
            }

            model_entry.tokens.input = model_entry
                .tokens
                .input
                .saturating_add(msg.tokens.input.max(0) as u64);
            model_entry.tokens.output = model_entry
                .tokens
                .output
                .saturating_add(msg.tokens.output.max(0) as u64);
            model_entry.tokens.cache_read = model_entry
                .tokens
                .cache_read
                .saturating_add(msg.tokens.cache_read.max(0) as u64);
            model_entry.tokens.cache_write = model_entry
                .tokens
                .cache_write
                .saturating_add(msg.tokens.cache_write.max(0) as u64);
            model_entry.tokens.reasoning = model_entry
                .tokens
                .reasoning
                .saturating_add(msg.tokens.reasoning.max(0) as u64);
            let msg_cost = if msg.cost.is_finite() && msg.cost >= 0.0 {
                msg.cost
            } else {
                0.0
            };
            model_entry.cost += msg_cost;

            let session_key = format!("{}:{}", msg.client, msg.session_id);
            let model_sessions = model_session_ids.entry(key).or_default();
            if model_sessions.insert(session_key) {
                model_entry.session_count += 1;
            }

            if let Some(agent) = msg.agent.as_ref() {
                let normalized_agent = if msg.client == "opencode" {
                    sessions::normalize_opencode_agent_name(agent)
                } else {
                    sessions::normalize_agent_name(agent)
                };
                let agent_entry = agent_map
                    .entry(normalized_agent.clone())
                    .or_insert_with(|| AgentUsage {
                        agent: normalized_agent.clone(),
                        clients: String::new(),
                        tokens: TokenBreakdown::default(),
                        cost: 0.0,
                        message_count: 0,
                    });

                agent_entry.tokens.input = agent_entry
                    .tokens
                    .input
                    .saturating_add(msg.tokens.input.max(0) as u64);
                agent_entry.tokens.output = agent_entry
                    .tokens
                    .output
                    .saturating_add(msg.tokens.output.max(0) as u64);
                agent_entry.tokens.cache_read = agent_entry
                    .tokens
                    .cache_read
                    .saturating_add(msg.tokens.cache_read.max(0) as u64);
                agent_entry.tokens.cache_write = agent_entry
                    .tokens
                    .cache_write
                    .saturating_add(msg.tokens.cache_write.max(0) as u64);
                agent_entry.tokens.reasoning = agent_entry
                    .tokens
                    .reasoning
                    .saturating_add(msg.tokens.reasoning.max(0) as u64);
                agent_entry.cost += msg_cost;
                agent_entry.message_count = agent_entry
                    .message_count
                    .saturating_add(msg.message_count.max(0) as u32);

                agent_clients
                    .entry(normalized_agent)
                    .or_default()
                    .insert(msg.client.clone());
            }

            if let Some(date) = parse_date(&msg.date) {
                let daily_entry = daily_map.entry(date).or_insert_with(|| DailyUsage {
                    date,
                    tokens: TokenBreakdown::default(),
                    cost: 0.0,
                    source_breakdown: BTreeMap::new(),
                    message_count: 0,
                    turn_count: 0,
                });

                daily_entry.tokens.input = daily_entry
                    .tokens
                    .input
                    .saturating_add(msg.tokens.input.max(0) as u64);
                daily_entry.tokens.output = daily_entry
                    .tokens
                    .output
                    .saturating_add(msg.tokens.output.max(0) as u64);
                daily_entry.tokens.cache_read = daily_entry
                    .tokens
                    .cache_read
                    .saturating_add(msg.tokens.cache_read.max(0) as u64);
                daily_entry.tokens.cache_write = daily_entry
                    .tokens
                    .cache_write
                    .saturating_add(msg.tokens.cache_write.max(0) as u64);
                daily_entry.tokens.reasoning = daily_entry
                    .tokens
                    .reasoning
                    .saturating_add(msg.tokens.reasoning.max(0) as u64);
                let msg_cost = if msg.cost.is_finite() && msg.cost >= 0.0 {
                    msg.cost
                } else {
                    0.0
                };
                daily_entry.cost += msg_cost;
                daily_entry.message_count += msg.message_count.max(0) as u32;
                if msg.is_turn_start {
                    daily_entry.turn_count += 1;
                }

                let source_entry = daily_entry
                    .source_breakdown
                    .entry(msg.client.clone())
                    .or_insert_with(|| DailySourceInfo {
                        tokens: TokenBreakdown::default(),
                        cost: 0.0,
                        models: BTreeMap::new(),
                    });

                source_entry.tokens.input = source_entry
                    .tokens
                    .input
                    .saturating_add(msg.tokens.input.max(0) as u64);
                source_entry.tokens.output = source_entry
                    .tokens
                    .output
                    .saturating_add(msg.tokens.output.max(0) as u64);
                source_entry.tokens.cache_read = source_entry
                    .tokens
                    .cache_read
                    .saturating_add(msg.tokens.cache_read.max(0) as u64);
                source_entry.tokens.cache_write = source_entry
                    .tokens
                    .cache_write
                    .saturating_add(msg.tokens.cache_write.max(0) as u64);
                source_entry.tokens.reasoning = source_entry
                    .tokens
                    .reasoning
                    .saturating_add(msg.tokens.reasoning.max(0) as u64);
                source_entry.cost += msg_cost;

                let daily_model_key = daily_source_model_key(
                    group_by,
                    &workspace_group_key,
                    &msg.provider_id,
                    &normalized_model,
                );

                let model_info = source_entry
                    .models
                    .entry(daily_model_key)
                    .or_insert_with(|| DailyModelInfo {
                        provider: msg.provider_id.clone(),
                        display_name: daily_source_model_display_name(
                            group_by,
                            &workspace_label,
                            &msg.provider_id,
                            &normalized_model,
                        ),
                        color_key: model_color_key(group_by, &msg.provider_id, &normalized_model),
                        tokens: TokenBreakdown::default(),
                        cost: 0.0,
                        messages: 0,
                    });

                model_info.tokens.input = model_info
                    .tokens
                    .input
                    .saturating_add(msg.tokens.input.max(0) as u64);
                model_info.tokens.output = model_info
                    .tokens
                    .output
                    .saturating_add(msg.tokens.output.max(0) as u64);
                model_info.tokens.cache_read = model_info
                    .tokens
                    .cache_read
                    .saturating_add(msg.tokens.cache_read.max(0) as u64);
                model_info.tokens.cache_write = model_info
                    .tokens
                    .cache_write
                    .saturating_add(msg.tokens.cache_write.max(0) as u64);
                model_info.tokens.reasoning = model_info
                    .tokens
                    .reasoning
                    .saturating_add(msg.tokens.reasoning.max(0) as u64);
                model_info.cost += msg_cost;
                model_info.messages = model_info
                    .messages
                    .saturating_add(msg.message_count.max(0) as u64);
            }

            // Hourly aggregation: derive hour from timestamp (Unix ms),
            // falling back to msg.date 00:00 when timestamp is missing/zero
            // so we don't silently drop messages (matches CLI bucketing).
            if let Some(hour_dt) = hour_bucket_with_fallback(msg.timestamp, &msg.date) {
                let hourly_entry = hourly_map.entry(hour_dt).or_insert_with(|| HourlyUsage {
                    datetime: hour_dt,
                    tokens: TokenBreakdown::default(),
                    cost: 0.0,
                    clients: BTreeSet::new(),
                    models: BTreeMap::new(),
                    message_count: 0,
                    turn_count: 0,
                });

                hourly_entry.tokens.input = hourly_entry
                    .tokens
                    .input
                    .saturating_add(msg.tokens.input.max(0) as u64);
                hourly_entry.tokens.output = hourly_entry
                    .tokens
                    .output
                    .saturating_add(msg.tokens.output.max(0) as u64);
                hourly_entry.tokens.cache_read = hourly_entry
                    .tokens
                    .cache_read
                    .saturating_add(msg.tokens.cache_read.max(0) as u64);
                hourly_entry.tokens.cache_write = hourly_entry
                    .tokens
                    .cache_write
                    .saturating_add(msg.tokens.cache_write.max(0) as u64);
                hourly_entry.tokens.reasoning = hourly_entry
                    .tokens
                    .reasoning
                    .saturating_add(msg.tokens.reasoning.max(0) as u64);
                let h_cost = if msg.cost.is_finite() && msg.cost >= 0.0 {
                    msg.cost
                } else {
                    0.0
                };
                hourly_entry.cost += h_cost;
                hourly_entry.message_count += msg.message_count.max(0) as u32;
                if msg.is_turn_start {
                    hourly_entry.turn_count += 1;
                }
                hourly_entry.clients.insert(msg.client.clone());

                let hourly_model_key =
                    hourly_model_key(group_by, &msg.provider_id, &normalized_model);
                let h_model = hourly_entry
                    .models
                    .entry(hourly_model_key)
                    .or_insert_with(|| HourlyModelInfo {
                        provider: msg.provider_id.clone(),
                        display_name: hourly_model_display_name(
                            group_by,
                            &msg.provider_id,
                            &normalized_model,
                        ),
                        color_key: model_color_key(group_by, &msg.provider_id, &normalized_model),
                        tokens: TokenBreakdown::default(),
                        cost: 0.0,
                    });
                h_model.tokens.input = h_model
                    .tokens
                    .input
                    .saturating_add(msg.tokens.input.max(0) as u64);
                h_model.tokens.output = h_model
                    .tokens
                    .output
                    .saturating_add(msg.tokens.output.max(0) as u64);
                h_model.tokens.cache_read = h_model
                    .tokens
                    .cache_read
                    .saturating_add(msg.tokens.cache_read.max(0) as u64);
                h_model.tokens.cache_write = h_model
                    .tokens
                    .cache_write
                    .saturating_add(msg.tokens.cache_write.max(0) as u64);
                h_model.tokens.reasoning = h_model
                    .tokens
                    .reasoning
                    .saturating_add(msg.tokens.reasoning.max(0) as u64);
                h_model.cost += h_cost;
            }
        }

        let mut models: Vec<ModelUsage> = model_map.into_values().collect();
        models.sort_by(|a, b| {
            b.cost
                .total_cmp(&a.cost)
                .then_with(|| a.model.cmp(&b.model))
                .then_with(|| a.provider.cmp(&b.provider))
        });

        for (agent, clients) in agent_clients {
            if let Some(agent_entry) = agent_map.get_mut(&agent) {
                agent_entry.clients = clients.into_iter().collect::<Vec<_>>().join(", ");
            }
        }

        let mut agents: Vec<AgentUsage> = agent_map.into_values().collect();
        agents.sort_by(|a, b| {
            b.cost
                .total_cmp(&a.cost)
                .then_with(|| b.tokens.total().cmp(&a.tokens.total()))
                .then_with(|| a.agent.cmp(&b.agent))
        });

        let mut daily: Vec<DailyUsage> = daily_map.into_values().collect();
        daily.sort_by_key(|b| std::cmp::Reverse(b.date));

        let mut hourly: Vec<HourlyUsage> = hourly_map.into_values().collect();
        hourly.sort_by_key(|b| std::cmp::Reverse(b.datetime));

        let total_tokens: u64 = models.iter().map(|m| m.tokens.total()).sum();
        let total_cost: f64 = models
            .iter()
            .map(|m| if m.cost.is_finite() { m.cost } else { 0.0 })
            .sum();

        let graph = build_contribution_graph(&daily);
        let (current_streak, longest_streak) = calculate_streaks(&daily);

        Ok(UsageData {
            models,
            agents,
            daily,
            hourly,
            prices,
            prices_daily,
            thinking,
            thinking_daily,
            speeds,
            speeds_daily,
            codex_accounts,
            quota_value,
            graph: Some(graph),
            total_tokens,
            total_cost,
            loading: false,
            error: None,
            current_streak,
            longest_streak,
        })
    }
}

async fn load_pricing_for_display() -> Option<Arc<PricingService>> {
    if std::env::var("TOKSCALE_PRICING_CACHE_ONLY")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
    {
        return PricingService::load_cached_any_age().map(Arc::new);
    }

    PricingService::get_or_init()
        .await
        .ok()
        .or_else(|| PricingService::load_cached_any_age().map(Arc::new))
}

fn build_price_rows(
    messages: &[UnifiedMessage],
    pricing: Option<&PricingService>,
) -> Vec<PriceUsage> {
    let mut price_map: HashMap<(NaiveDate, String), PriceUsage> = HashMap::new();

    for msg in messages {
        let Some(date) = parse_date(&msg.date) else {
            continue;
        };

        let lookup = pricing.and_then(|svc| {
            svc.lookup_with_source_and_provider(&msg.model_id, None, Some(&msg.provider_id))
        });
        let canonical_model = canonical_price_model(
            &msg.model_id,
            lookup.as_ref().map(|r| r.matched_key.as_str()),
        );
        let price_category = canonical_price_category(
            &msg.model_id,
            lookup.as_ref().map(|r| r.matched_key.as_str()),
        );
        let key = (date, price_category.clone());
        let entry = price_map.entry(key).or_insert_with(|| PriceUsage {
            date,
            model: canonical_model,
            provider: msg.provider_id.clone(),
            pricing_source: lookup.as_ref().map(|result| result.source.clone()),
            matched_key: Some(price_category.clone()),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            clients: BTreeSet::new(),
            message_count: 0,
            input_price_per_million: lookup
                .as_ref()
                .and_then(|result| result.pricing.input_cost_per_token)
                .map(|price| price * 1_000_000.0),
            output_price_per_million: lookup
                .as_ref()
                .and_then(|result| result.pricing.output_cost_per_token)
                .map(|price| price * 1_000_000.0),
            cache_read_price_per_million: lookup
                .as_ref()
                .and_then(|result| result.pricing.cache_read_input_token_cost)
                .map(|price| price * 1_000_000.0),
            cache_write_price_per_million: lookup
                .as_ref()
                .and_then(|result| result.pricing.cache_creation_input_token_cost)
                .map(|price| price * 1_000_000.0),
        });

        if !entry
            .provider
            .split(", ")
            .any(|provider| provider == msg.provider_id)
        {
            entry.provider = format!("{}, {}", entry.provider, msg.provider_id);
        }
        match (
            entry.pricing_source.as_deref(),
            lookup.as_ref().map(|result| result.source.as_str()),
        ) {
            (None, Some(source)) => entry.pricing_source = Some(source.to_string()),
            (Some(existing), Some(source)) if existing != source && existing != "Mixed" => {
                entry.pricing_source = Some("Mixed".to_string())
            }
            _ => {}
        }

        entry.tokens.input = entry
            .tokens
            .input
            .saturating_add(msg.tokens.input.max(0) as u64);
        entry.tokens.output = entry
            .tokens
            .output
            .saturating_add(msg.tokens.output.max(0) as u64);
        entry.tokens.cache_read = entry
            .tokens
            .cache_read
            .saturating_add(msg.tokens.cache_read.max(0) as u64);
        entry.tokens.cache_write = entry
            .tokens
            .cache_write
            .saturating_add(msg.tokens.cache_write.max(0) as u64);
        entry.tokens.reasoning = entry
            .tokens
            .reasoning
            .saturating_add(msg.tokens.reasoning.max(0) as u64);
        entry.cost += if msg.cost.is_finite() && msg.cost >= 0.0 {
            msg.cost
        } else {
            0.0
        };
        entry.clients.insert(msg.client.clone());
        entry.message_count = entry
            .message_count
            .saturating_add(msg.message_count.max(0) as u32);
    }

    let mut prices: Vec<PriceUsage> = price_map.into_values().collect();
    prices.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| b.cost.total_cmp(&a.cost))
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    prices
}

fn build_price_summaries(prices_daily: &[PriceUsage]) -> Vec<PriceSummary> {
    let mut summary_map: HashMap<String, PriceSummary> = HashMap::new();

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
                clients: BTreeSet::new(),
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

    let mut summaries: Vec<PriceSummary> = summary_map.into_values().collect();
    summaries.sort_by(|a, b| a.model.cmp(&b.model));
    summaries
}

fn canonical_price_model(model_id: &str, matched_key: Option<&str>) -> String {
    normalize_model_for_grouping(
        matched_key
            .map(strip_pricing_provider_prefix)
            .filter(|value| !value.is_empty())
            .unwrap_or(model_id),
    )
}

fn canonical_price_category(model_id: &str, matched_key: Option<&str>) -> String {
    matched_key
        .map(strip_pricing_provider_prefix)
        .filter(|value| !value.is_empty())
        .map(normalize_model_for_grouping)
        .unwrap_or_else(|| normalize_model_for_grouping(model_id))
}

fn strip_pricing_provider_prefix(key: &str) -> &str {
    key.rsplit_once('/')
        .map(|(_, model)| model)
        .filter(|model| !model.is_empty())
        .unwrap_or(key)
}

fn build_thinking_rows(messages: &[UnifiedMessage]) -> Vec<ThinkingUsage> {
    let mut thinking_map: HashMap<(NaiveDate, String, String, String), ThinkingUsage> =
        HashMap::new();

    for msg in messages {
        if msg.tokens.output <= 0 && msg.tokens.reasoning <= 0 {
            continue;
        }

        let Some(date) = parse_date(&msg.date) else {
            continue;
        };

        let normalized_model = normalize_model_for_grouping(&msg.model_id);
        let (base_model, thinking_level) = split_thinking_level(&normalized_model);
        let key = (
            date,
            msg.provider_id.clone(),
            base_model.clone(),
            thinking_level.clone(),
        );

        let entry = thinking_map.entry(key).or_insert_with(|| ThinkingUsage {
            date,
            model: base_model,
            provider: msg.provider_id.clone(),
            thinking_level,
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            clients: BTreeSet::new(),
            message_count: 0,
        });

        entry.tokens.input = entry
            .tokens
            .input
            .saturating_add(msg.tokens.input.max(0) as u64);
        entry.tokens.output = entry
            .tokens
            .output
            .saturating_add(msg.tokens.output.max(0) as u64);
        entry.tokens.cache_read = entry
            .tokens
            .cache_read
            .saturating_add(msg.tokens.cache_read.max(0) as u64);
        entry.tokens.cache_write = entry
            .tokens
            .cache_write
            .saturating_add(msg.tokens.cache_write.max(0) as u64);
        entry.tokens.reasoning = entry
            .tokens
            .reasoning
            .saturating_add(msg.tokens.reasoning.max(0) as u64);
        entry.cost += if msg.cost.is_finite() && msg.cost >= 0.0 {
            msg.cost
        } else {
            0.0
        };
        entry.clients.insert(msg.client.clone());
        entry.message_count = entry
            .message_count
            .saturating_add(msg.message_count.max(0) as u32);
    }

    let mut thinking: Vec<ThinkingUsage> = thinking_map.into_values().collect();
    thinking.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| {
                (b.tokens.output.saturating_add(b.tokens.reasoning))
                    .cmp(&a.tokens.output.saturating_add(a.tokens.reasoning))
            })
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.thinking_level.cmp(&b.thinking_level))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    thinking
}

fn build_thinking_summaries(thinking_daily: &[ThinkingUsage]) -> Vec<ThinkingSummary> {
    let mut summary_map: HashMap<String, ThinkingSummary> = HashMap::new();
    let mut trend_windows: HashMap<String, (NaiveDate, TokenBreakdown)> = HashMap::new();

    for row in thinking_daily {
        let entry = summary_map
            .entry(row.model.clone())
            .or_insert_with(|| ThinkingSummary {
                model: row.model.clone(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                clients: BTreeSet::new(),
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

        let trend_entry = trend_windows
            .entry(row.model.clone())
            .or_insert_with(|| (row.date, TokenBreakdown::default()));
        if row.date > trend_entry.0 {
            trend_entry.0 = row.date;
        }
    }

    for row in thinking_daily {
        let Some((latest_date, _)) = trend_windows.get(&row.model) else {
            continue;
        };
        let recent_start = latest_date
            .checked_sub_signed(Duration::days(29))
            .unwrap_or(*latest_date);
        let previous_start = recent_start
            .checked_sub_signed(Duration::days(30))
            .unwrap_or(recent_start);

        let trend_entry = trend_windows.get_mut(&row.model).expect("entry exists");
        if row.date >= previous_start && row.date < recent_start {
            trend_entry.1.output = trend_entry.1.output.saturating_add(row.tokens.output);
            trend_entry.1.reasoning = trend_entry.1.reasoning.saturating_add(row.tokens.reasoning);
        }
    }

    for summary in summary_map.values_mut() {
        let Some((latest_date, previous_tokens)) = trend_windows.get(&summary.model) else {
            continue;
        };
        let recent_start = latest_date
            .checked_sub_signed(Duration::days(29))
            .unwrap_or(*latest_date);
        let mut recent_tokens = TokenBreakdown::default();

        for row in thinking_daily
            .iter()
            .filter(|row| row.model == summary.model)
        {
            if row.date >= recent_start && row.date <= *latest_date {
                recent_tokens.output = recent_tokens.output.saturating_add(row.tokens.output);
                recent_tokens.reasoning =
                    recent_tokens.reasoning.saturating_add(row.tokens.reasoning);
            }
        }

        summary.thirty_day_trend_pct =
            calculate_thirty_day_trend_pct(&recent_tokens, previous_tokens);
    }

    let mut summaries: Vec<ThinkingSummary> = summary_map.into_values().collect();
    summaries.sort_by(|a, b| a.model.cmp(&b.model));
    summaries
}

fn build_speed_rows(messages: &[UnifiedMessage]) -> Vec<SpeedUsage> {
    let mut speed_map: HashMap<(NaiveDate, String, String, String), SpeedUsage> = HashMap::new();

    for msg in messages {
        let generated_tokens = msg
            .tokens
            .output
            .max(0)
            .saturating_add(msg.tokens.reasoning.max(0)) as u64;
        let Some(duration_ms) = msg.generation_duration_ms else {
            continue;
        };
        if !has_reasonable_tokens_per_second(generated_tokens, duration_ms) {
            continue;
        }

        let Some(date) = parse_date(&msg.date) else {
            continue;
        };

        let normalized_model = normalize_model_for_grouping(&msg.model_id);
        let (base_model, thinking_level) = split_thinking_level(&normalized_model);
        let key = (
            date,
            msg.provider_id.clone(),
            base_model.clone(),
            thinking_level.clone(),
        );

        let entry = speed_map.entry(key).or_insert_with(|| SpeedUsage {
            date,
            model: base_model,
            provider: msg.provider_id.clone(),
            thinking_level,
            generated_tokens: 0,
            generation_duration_ms: 0,
            clients: BTreeSet::new(),
            sample_count: 0,
        });

        entry.generated_tokens = entry.generated_tokens.saturating_add(generated_tokens);
        entry.generation_duration_ms = entry.generation_duration_ms.saturating_add(duration_ms);
        entry.clients.insert(msg.client.clone());
        entry.sample_count = entry.sample_count.saturating_add(1);
    }

    let mut speeds: Vec<SpeedUsage> = speed_map.into_values().collect();
    speeds.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| b.tokens_per_second().total_cmp(&a.tokens_per_second()))
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.thinking_level.cmp(&b.thinking_level))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    speeds
}

fn build_speed_summaries(speeds_daily: &[SpeedUsage]) -> Vec<SpeedSummary> {
    let mut summary_map: HashMap<(String, String, String), SpeedSummary> = HashMap::new();

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
            clients: BTreeSet::new(),
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

fn calculate_thirty_day_trend_pct(
    recent: &TokenBreakdown,
    previous: &TokenBreakdown,
) -> Option<f64> {
    let recent_generated = recent.output.saturating_add(recent.reasoning);
    let previous_generated = previous.output.saturating_add(previous.reasoning);
    if recent_generated == 0 && previous_generated == 0 {
        return Some(0.0);
    }
    if previous_generated == 0 || previous.reasoning == 0 {
        return None;
    }

    let recent_rate = recent.reasoning as f64 / recent_generated as f64;
    let previous_rate = previous.reasoning as f64 / previous_generated as f64;
    if previous_rate <= f64::EPSILON {
        return None;
    }

    Some(((recent_rate - previous_rate) / previous_rate) * 100.0)
}

fn build_codex_account_summaries(messages: &[UnifiedMessage]) -> Vec<CodexAccountUsage> {
    let mut account_map: HashMap<String, CodexAccountUsage> = HashMap::new();
    let mut account_sessions: HashMap<String, HashSet<String>> = HashMap::new();
    let mut account_months: HashMap<String, HashSet<String>> = HashMap::new();

    for msg in messages {
        if msg.client != "codex" {
            continue;
        }

        let account_hash = msg
            .codex_account_hash
            .clone()
            .unwrap_or_else(|| UNATTRIBUTED_CODEX_ACCOUNT.to_string());
        let tracks_subscription = is_billable_codex_account_bucket(&account_hash);
        let entry = account_map
            .entry(account_hash.clone())
            .or_insert_with(|| CodexAccountUsage {
                account_hash: account_hash.clone(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                paid_cost: tracks_subscription.then_some(0.0),
                active_month_count: tracks_subscription.then_some(0),
                message_count: 0,
                turn_count: 0,
                session_count: 0,
                first_date: None,
                latest_date: None,
            });

        entry.tokens.input = entry
            .tokens
            .input
            .saturating_add(msg.tokens.input.max(0) as u64);
        entry.tokens.output = entry
            .tokens
            .output
            .saturating_add(msg.tokens.output.max(0) as u64);
        entry.tokens.cache_read = entry
            .tokens
            .cache_read
            .saturating_add(msg.tokens.cache_read.max(0) as u64);
        entry.tokens.cache_write = entry
            .tokens
            .cache_write
            .saturating_add(msg.tokens.cache_write.max(0) as u64);
        entry.tokens.reasoning = entry
            .tokens
            .reasoning
            .saturating_add(msg.tokens.reasoning.max(0) as u64);
        entry.cost += if msg.cost.is_finite() && msg.cost >= 0.0 {
            msg.cost
        } else {
            0.0
        };
        entry.message_count = entry
            .message_count
            .saturating_add(msg.message_count.max(0) as u32);
        if msg.is_turn_start {
            entry.turn_count = entry.turn_count.saturating_add(1);
        }
        if let Some(date) = parse_date(&msg.date) {
            entry.first_date = Some(entry.first_date.map_or(date, |current| current.min(date)));
            entry.latest_date = Some(entry.latest_date.map_or(date, |current| current.max(date)));
            if tracks_subscription {
                let months = account_months.entry(account_hash.clone()).or_default();
                if months.insert(format!("{}-{:02}", date.year(), date.month())) {
                    if let Some(month_count) = entry.active_month_count.as_mut() {
                        *month_count = month_count.saturating_add(1);
                    }
                    if let Some(paid_cost) = entry.paid_cost.as_mut() {
                        *paid_cost += CODEX_MONTHLY_SUBSCRIPTION_COST_USD;
                    }
                }
            }
        }

        let sessions = account_sessions.entry(account_hash).or_default();
        if sessions.insert(msg.session_id.clone()) {
            entry.session_count = entry.session_count.saturating_add(1);
        }
    }

    let mut accounts: Vec<CodexAccountUsage> = account_map.into_values().collect();
    accounts.sort_by(|a, b| {
        b.cost
            .total_cmp(&a.cost)
            .then_with(|| b.tokens.total().cmp(&a.tokens.total()))
            .then_with(|| a.account_hash.cmp(&b.account_hash))
    });
    accounts
}

fn build_quota_value_data(
    messages: &[UnifiedMessage],
    samples: &[CodexQuotaSample],
) -> QuotaValueData {
    if samples.is_empty() {
        return QuotaValueData::default();
    }

    let mut grouped_samples: BTreeMap<(String, String, i64, i64), Vec<&CodexQuotaSample>> =
        BTreeMap::new();
    for sample in samples {
        let account_hash = sample
            .codex_account_hash
            .clone()
            .unwrap_or_else(|| UNATTRIBUTED_CODEX_ACCOUNT.to_string());
        grouped_samples
            .entry((
                account_hash,
                sample.window_kind.clone(),
                sample.resets_at,
                sample.window_minutes,
            ))
            .or_default()
            .push(sample);
    }

    let mut intervals = Vec::new();
    for ((account_hash, window_kind, _resets_at, window_minutes), mut group) in grouped_samples {
        group.sort_by_key(|sample| sample.timestamp);
        for pair in group.windows(2) {
            let start = pair[0];
            let end = pair[1];
            let quota_burn_pct = end.used_percent - start.used_percent;
            if quota_burn_pct <= 0.0 || !quota_burn_pct.is_finite() {
                continue;
            }

            let mut per_model: BTreeMap<String, (TokenBreakdown, f64)> = BTreeMap::new();
            let mut models = BTreeSet::new();
            for message in messages {
                if message.client != "codex" {
                    continue;
                }
                let message_account = message
                    .codex_account_hash
                    .clone()
                    .unwrap_or_else(|| UNATTRIBUTED_CODEX_ACCOUNT.to_string());
                if message_account != account_hash {
                    continue;
                }
                if message.timestamp <= start.timestamp || message.timestamp > end.timestamp {
                    continue;
                }

                let model = normalize_model_for_grouping(&message.model_id);
                let (tokens, api_value_usd) = per_model.entry(model.clone()).or_default();
                tokens.input = tokens
                    .input
                    .saturating_add(message.tokens.input.max(0) as u64);
                tokens.output = tokens
                    .output
                    .saturating_add(message.tokens.output.max(0) as u64);
                tokens.cache_read = tokens
                    .cache_read
                    .saturating_add(message.tokens.cache_read.max(0) as u64);
                tokens.cache_write = tokens
                    .cache_write
                    .saturating_add(message.tokens.cache_write.max(0) as u64);
                tokens.reasoning = tokens
                    .reasoning
                    .saturating_add(message.tokens.reasoning.max(0) as u64);
                if message.cost.is_finite() && message.cost > 0.0 {
                    *api_value_usd += message.cost;
                }
                models.insert(model);
            }

            if let Some(end_dt) = timestamp_to_local_datetime(end.timestamp) {
                let total_tokens = per_model
                    .values()
                    .map(|(tokens, _)| tokens.total())
                    .sum::<u64>();
                let model_count = per_model.len().max(1) as f64;
                for (model, (tokens, api_value_usd)) in per_model {
                    let token_share = if total_tokens > 0 {
                        tokens.total() as f64 / total_tokens as f64
                    } else {
                        1.0 / model_count
                    };
                    let model_quota_burn_pct = quota_burn_pct * token_share;
                    let subscription_cost_burned =
                        subscription_cost_burned(window_minutes, model_quota_burn_pct);
                    let factor = positive_ratio(api_value_usd, subscription_cost_burned);
                    let dollars_per_percent = positive_ratio(api_value_usd, model_quota_burn_pct);
                    intervals.push(QuotaValueInterval {
                        account_hash: account_hash.clone(),
                        model,
                        window_kind: window_kind.clone(),
                        end: end_dt,
                        quota_burn_pct: model_quota_burn_pct,
                        api_value_usd,
                        subscription_cost_burned,
                        factor,
                        dollars_per_percent,
                        tokens,
                        models: models.clone(),
                        sample_count: 2,
                        confidence: QuotaConfidence::Low,
                    });
                }
            }
        }
    }

    intervals.sort_by_key(|interval| interval.end);
    let points = build_quota_value_points(&intervals);
    let model_summaries = build_quota_model_summaries(&intervals, &points);
    let point_confidence: HashMap<(NaiveDate, String, String), QuotaConfidence> = points
        .iter()
        .map(|point| {
            (
                (point.date, point.model.clone(), point.window_kind.clone()),
                point.confidence,
            )
        })
        .collect();
    for interval in &mut intervals {
        if let Some(confidence) = point_confidence.get(&(
            interval.end.date(),
            interval.model.clone(),
            interval.window_kind.clone(),
        )) {
            interval.confidence = *confidence;
        }
    }
    intervals.sort_by_key(|interval| std::cmp::Reverse(interval.end));

    QuotaValueData {
        intervals,
        points,
        model_summaries,
        sample_count: samples.len() as u32,
    }
}

fn build_quota_value_points(intervals: &[QuotaValueInterval]) -> Vec<QuotaValuePoint> {
    let mut dates = BTreeSet::new();
    let mut models = BTreeSet::new();
    let mut window_kinds = BTreeSet::new();
    for interval in intervals {
        dates.insert(interval.end.date());
        models.insert(interval.model.clone());
        window_kinds.insert(interval.window_kind.clone());
    }

    let mut points = Vec::new();
    for date in dates {
        let start = date - Duration::days(3);
        for model in &models {
            for window_kind in &window_kinds {
                let window: Vec<&QuotaValueInterval> = intervals
                    .iter()
                    .filter(|interval| {
                        interval.model == *model
                            && interval.window_kind == *window_kind
                            && interval.end.date() >= start
                            && interval.end.date() <= date
                    })
                    .collect();
                if window.is_empty() {
                    continue;
                }
                let quota_burn_pct: f64 =
                    window.iter().map(|interval| interval.quota_burn_pct).sum();
                let api_value_usd: f64 = window.iter().map(|interval| interval.api_value_usd).sum();
                let subscription_cost_burned: f64 = window
                    .iter()
                    .map(|interval| interval.subscription_cost_burned)
                    .sum();
                let interval_count = window.len() as u32;
                points.push(QuotaValuePoint {
                    date,
                    model: model.clone(),
                    window_kind: window_kind.clone(),
                    quota_burn_pct,
                    api_value_usd,
                    subscription_cost_burned,
                    factor: positive_ratio(api_value_usd, subscription_cost_burned),
                    dollars_per_percent: positive_ratio(api_value_usd, quota_burn_pct),
                    interval_count,
                    confidence: quota_confidence(interval_count, quota_burn_pct),
                });
            }
        }
    }
    points.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.window_kind.cmp(&b.window_kind))
    });
    points
}

fn build_quota_model_summaries(
    intervals: &[QuotaValueInterval],
    points: &[QuotaValuePoint],
) -> Vec<QuotaModelSummary> {
    let mut latest_by_model: BTreeMap<String, &QuotaValuePoint> = BTreeMap::new();
    for point in points {
        let current = latest_by_model.get(&point.model).copied();
        let should_replace = match current {
            None => true,
            Some(existing) => {
                (point.window_kind == "secondary" && existing.window_kind != "secondary")
                    || (point.window_kind == existing.window_kind && point.date > existing.date)
                    || (point.date > existing.date && existing.window_kind != "secondary")
            }
        };
        if should_replace {
            latest_by_model.insert(point.model.clone(), point);
        }
    }

    let mut summaries = Vec::new();
    for point in latest_by_model.into_values() {
        let start = point.date - Duration::days(3);
        let mut tokens = TokenBreakdown::default();
        for interval in intervals {
            if interval.model != point.model
                || interval.window_kind != point.window_kind
                || interval.end.date() < start
                || interval.end.date() > point.date
            {
                continue;
            }
            tokens.input = tokens.input.saturating_add(interval.tokens.input);
            tokens.output = tokens.output.saturating_add(interval.tokens.output);
            tokens.cache_read = tokens.cache_read.saturating_add(interval.tokens.cache_read);
            tokens.cache_write = tokens
                .cache_write
                .saturating_add(interval.tokens.cache_write);
            tokens.reasoning = tokens.reasoning.saturating_add(interval.tokens.reasoning);
        }

        summaries.push(QuotaModelSummary {
            model: point.model.clone(),
            window_kind: point.window_kind.clone(),
            latest_date: point.date,
            quota_burn_pct: point.quota_burn_pct,
            api_value_usd: point.api_value_usd,
            subscription_cost_burned: point.subscription_cost_burned,
            factor: point.factor,
            dollars_per_percent: point.dollars_per_percent,
            tokens,
            interval_count: point.interval_count,
            confidence: point.confidence,
        });
    }
    summaries.sort_by(|a, b| {
        b.api_value_usd
            .total_cmp(&a.api_value_usd)
            .then_with(|| b.factor.unwrap_or(0.0).total_cmp(&a.factor.unwrap_or(0.0)))
            .then_with(|| a.model.cmp(&b.model))
    });
    summaries
}

fn positive_ratio(numerator: f64, denominator: f64) -> Option<f64> {
    (numerator.is_finite() && denominator.is_finite() && denominator > 0.0)
        .then_some(numerator / denominator)
}

fn subscription_cost_burned(window_minutes: i64, quota_burn_pct: f64) -> f64 {
    const AVERAGE_MONTH_MINUTES: f64 = 365.2425 / 12.0 * 24.0 * 60.0;
    let window_budget =
        CODEX_MONTHLY_SUBSCRIPTION_COST_USD * window_minutes as f64 / AVERAGE_MONTH_MINUTES;
    window_budget * quota_burn_pct / 100.0
}

fn quota_confidence(interval_count: u32, quota_burn_pct: f64) -> QuotaConfidence {
    if interval_count >= 3 && quota_burn_pct >= 2.0 {
        QuotaConfidence::High
    } else if interval_count >= 2 && quota_burn_pct >= 1.0 {
        QuotaConfidence::Medium
    } else {
        QuotaConfidence::Low
    }
}

fn timestamp_to_local_datetime(timestamp_ms: i64) -> Option<NaiveDateTime> {
    use chrono::TimeZone;
    if timestamp_ms <= 0 {
        return None;
    }
    match Local.timestamp_millis_opt(timestamp_ms) {
        chrono::LocalResult::Single(dt) => Some(dt.naive_local()),
        _ => None,
    }
}

fn split_thinking_level(model: &str) -> (String, String) {
    let lower = model.to_lowercase();

    if let Some((base, tail)) = split_suffix_tail(model, &lower, "-thinking-")
        .or_else(|| split_suffix_tail(model, &lower, "_thinking_"))
    {
        return (base, format!("Thinking {}", format_effort_label(&tail)));
    }

    for needle in ["-thinking", "_thinking"] {
        if lower.ends_with(needle) {
            return (
                model[..model.len() - needle.len()].to_string(),
                "Thinking".to_string(),
            );
        }
    }

    for (needle, label) in [
        ("-xhigh", "XHigh"),
        ("_xhigh", "XHigh"),
        ("-high", "High"),
        ("_high", "High"),
        ("-medium", "Medium"),
        ("_medium", "Medium"),
        ("-low", "Low"),
        ("_low", "Low"),
    ] {
        if lower.ends_with(needle) {
            return (
                model[..model.len() - needle.len()].to_string(),
                label.to_string(),
            );
        }
    }

    (model.to_string(), "Default".to_string())
}

fn split_suffix_tail(model: &str, lower: &str, needle: &str) -> Option<(String, String)> {
    let idx = lower.rfind(needle)?;
    let tail_start = idx + needle.len();
    if tail_start >= model.len() {
        return None;
    }
    let tail = &model[tail_start..];
    if tail.is_empty()
        || !tail
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return None;
    }
    Some((model[..idx].to_string(), tail.to_string()))
}

fn format_effort_label(tail: &str) -> String {
    tail.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            if part.chars().all(|ch| ch.is_ascii_digit()) {
                part.to_string()
            } else {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => {
                        let mut label = String::new();
                        label.push(first.to_ascii_uppercase());
                        label.push_str(&chars.as_str().to_ascii_lowercase());
                        label
                    }
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_date(date_str: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()
}

/// Convert Unix ms timestamp to a NaiveDateTime truncated to the hour (local tz).
fn timestamp_to_hour(timestamp_ms: i64) -> Option<NaiveDateTime> {
    use chrono::TimeZone;
    if timestamp_ms <= 0 {
        return None;
    }
    let ts_secs = timestamp_ms / 1000;
    match Local.timestamp_opt(ts_secs, 0) {
        chrono::LocalResult::Single(dt) => {
            let naive = dt.naive_local();
            Some(
                naive
                    .date()
                    .and_hms_opt(naive.hour(), 0, 0)
                    .unwrap_or(naive),
            )
        }
        _ => None,
    }
}

/// Derive an hour-truncated NaiveDateTime from `msg.timestamp` when present,
/// otherwise fall back to `msg.date`'s 00:00 bucket so messages with missing
/// timestamps are not silently dropped from hourly aggregation. Mirrors the
/// CLI hourly bucketing behavior in `tokscale-core::lib::get_hourly_report`.
fn hour_bucket_with_fallback(timestamp_ms: i64, date_str: &str) -> Option<NaiveDateTime> {
    if let Some(dt) = timestamp_to_hour(timestamp_ms) {
        return Some(dt);
    }
    parse_date(date_str).and_then(|d| d.and_hms_opt(0, 0, 0))
}

fn build_contribution_graph(daily: &[DailyUsage]) -> GraphData {
    build_contribution_graph_for_today(daily, Local::now().date_naive())
}

fn build_contribution_graph_for_today(daily: &[DailyUsage], today: NaiveDate) -> GraphData {
    if daily.is_empty() {
        return GraphData { weeks: vec![] };
    }

    let days_to_sunday = today.weekday().num_days_from_sunday();
    let end_date = today;
    let start_date = end_date - chrono::Duration::days(364 + days_to_sunday as i64);

    let daily_map: HashMap<NaiveDate, &DailyUsage> = daily.iter().map(|d| (d.date, d)).collect();

    let max_cost = daily.iter().map(|d| d.cost).fold(0.0_f64, |a, b| a.max(b));

    let mut weeks: Vec<Vec<Option<ContributionDay>>> = Vec::new();
    let mut current_week: Vec<Option<ContributionDay>> = Vec::new();

    let mut current_date = start_date;
    while current_date <= end_date {
        let day = if let Some(usage) = daily_map.get(&current_date) {
            let raw_intensity = if max_cost > 0.0 {
                usage.cost / max_cost
            } else {
                0.0
            };
            let intensity = if raw_intensity.is_finite() {
                raw_intensity.clamp(0.0, 1.0)
            } else {
                0.0
            };
            Some(ContributionDay {
                date: current_date,
                tokens: usage.tokens.total(),
                cost: usage.cost,
                intensity,
            })
        } else {
            Some(ContributionDay {
                date: current_date,
                tokens: 0,
                cost: 0.0,
                intensity: 0.0,
            })
        };

        current_week.push(day);

        if current_date.weekday() == chrono::Weekday::Sat || current_date == end_date {
            weeks.push(current_week);
            current_week = Vec::new();
        }

        current_date += chrono::Duration::days(1);
    }

    GraphData { weeks }
}

fn calculate_streaks(daily: &[DailyUsage]) -> (u32, u32) {
    calculate_streaks_for_today(daily, Local::now().date_naive())
}

fn calculate_streaks_for_today(daily: &[DailyUsage], today: NaiveDate) -> (u32, u32) {
    if daily.is_empty() {
        return (0, 0);
    }

    let dates: HashSet<NaiveDate> = daily.iter().map(|d| d.date).collect();

    let mut current_streak = 0u32;
    let mut check_date = today;

    while dates.contains(&check_date) {
        current_streak += 1;
        check_date -= chrono::Duration::days(1);
    }

    if current_streak == 0 {
        let yesterday = today - chrono::Duration::days(1);
        check_date = yesterday;
        while dates.contains(&check_date) {
            current_streak += 1;
            check_date -= chrono::Duration::days(1);
        }
    }

    let mut longest_streak = 0u32;
    let mut sorted_dates: Vec<NaiveDate> = dates.into_iter().collect();
    sorted_dates.sort();

    let mut streak = 0u32;
    let mut prev_date: Option<NaiveDate> = None;

    for date in sorted_dates {
        if let Some(prev) = prev_date {
            if date == prev + chrono::Duration::days(1) {
                streak += 1;
            } else {
                longest_streak = longest_streak.max(streak);
                streak = 1;
            }
        } else {
            streak = 1;
        }
        prev_date = Some(date);
    }
    longest_streak = longest_streak.max(streak);

    (current_streak, longest_streak)
}

/// Time-of-day period bucket for profile view
#[derive(Debug, Clone)]
pub struct PeriodBucket {
    pub label: &'static str,
    pub hour_range: &'static str,
    pub total_tokens: u64,
}

/// Weekday bucket for profile view
#[derive(Debug, Clone)]
pub struct WeekdayBucket {
    pub day: &'static str,
    pub total_tokens: u64,
}

/// Aggregate hourly data into time-of-day periods
pub fn aggregate_by_period(hourly: &[HourlyUsage]) -> Vec<PeriodBucket> {
    let periods: [(&str, &str, Vec<usize>); 4] = [
        ("Morning", "05:00-11:59", (5..=11).collect()),
        ("Daytime", "12:00-16:59", (12..=16).collect()),
        ("Evening", "17:00-21:59", (17..=21).collect()),
        ("Night", "22:00-04:59", vec![22, 23, 0, 1, 2, 3, 4]),
    ];

    periods
        .iter()
        .map(|(label, hour_range, hours)| {
            let mut total_tokens = 0u64;

            for entry in hourly {
                let hour = entry.datetime.hour() as usize;
                if hours.contains(&hour) {
                    total_tokens = total_tokens.saturating_add(entry.tokens.total());
                }
            }

            PeriodBucket {
                label,
                hour_range,
                total_tokens,
            }
        })
        .collect()
}

/// Aggregate hourly data by weekday
pub fn aggregate_by_weekday(hourly: &[HourlyUsage]) -> Vec<WeekdayBucket> {
    use chrono::Datelike;

    let weekdays = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    let mut buckets: Vec<u64> = vec![0; 7];

    for entry in hourly {
        let weekday = entry.datetime.weekday().num_days_from_monday() as usize;
        buckets[weekday] = buckets[weekday].saturating_add(entry.tokens.total());
    }

    weekdays
        .iter()
        .enumerate()
        .map(|(i, day)| WeekdayBucket {
            day,
            total_tokens: buckets[i],
        })
        .collect()
}

/// Find peak hour across all hourly data
pub fn find_peak_hour(hourly: &[HourlyUsage]) -> Option<(u32, u64, f64)> {
    use std::collections::HashMap;

    let mut hour_totals: HashMap<u32, (u64, f64)> = HashMap::new();

    for entry in hourly {
        let hour = entry.datetime.hour();
        let entry_totals = hour_totals.entry(hour).or_insert((0, 0.0));
        entry_totals.0 = entry_totals.0.saturating_add(entry.tokens.total());
        entry_totals.1 += entry.cost;
    }

    hour_totals
        .into_iter()
        .max_by_key(|(_, (tokens, _))| *tokens)
        .map(|(hour, (tokens, cost))| (hour, tokens, cost))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::collections::HashMap;
    use std::env;
    use std::fs;
    use tempfile::TempDir;
    use tokio::runtime::{Handle, Runtime};
    use tokscale_core::parse_local_usage_with_pricing;
    use tokscale_core::pricing::{ModelPricing, PricingService};
    use tokscale_core::TokenBreakdown as CoreTokenBreakdown;

    fn test_pricing_service() -> PricingService {
        let mut litellm = HashMap::new();
        litellm.insert(
            "claude-sonnet-4".into(),
            ModelPricing {
                input_cost_per_token: Some(0.00001),
                output_cost_per_token: Some(0.00002),
                cache_read_input_token_cost: Some(0.000003),
                ..Default::default()
            },
        );
        litellm.insert(
            "claude-haiku-4".into(),
            ModelPricing {
                input_cost_per_token: Some(0.000004),
                output_cost_per_token: Some(0.000006),
                cache_read_input_token_cost: Some(0.000001),
                ..Default::default()
            },
        );
        litellm.insert(
            "accounts/fireworks/models/deepseek-v3-0324".into(),
            ModelPricing {
                input_cost_per_token: Some(0.01),
                output_cost_per_token: Some(0.03),
                ..Default::default()
            },
        );

        PricingService::new(litellm, HashMap::new())
    }

    fn load_with_pricing(
        loader: &DataLoader,
        enabled_clients: &[ClientId],
        group_by: &GroupBy,
        include_synthetic: bool,
        pricing: Option<&PricingService>,
    ) -> Result<UsageData> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .to_string_lossy()
            .to_string();

        let mut sources: Vec<String> = enabled_clients
            .iter()
            .map(|client| client.as_str().to_string())
            .collect();
        if include_synthetic {
            sources.push("synthetic".to_string());
        }

        let opts = LocalParseOptions {
            home_dir: Some(home),
            use_env_roots: true,
            clients: Some(sources),
            since: loader.since.clone(),
            until: loader.until.clone(),
            year: loader.year.clone(),
            scanner_settings: data_loader_scanner_settings(),
        };

        let usage = if Handle::try_current().is_ok() {
            std::thread::scope(|s| {
                s.spawn(|| {
                    let rt = Runtime::new().map_err(|e| e.to_string())?;
                    rt.block_on(parse_local_usage_with_pricing(opts, pricing))
                })
                .join()
                .unwrap_or_else(|_| Err("data loader thread panicked".to_string()))
            })
        } else {
            Runtime::new()?.block_on(parse_local_usage_with_pricing(opts, pricing))
        }
        .map_err(anyhow::Error::msg)?;

        loader.aggregate_messages_with_pricing(
            usage.messages,
            usage.quota_samples,
            group_by,
            pricing,
        )
    }

    fn expected_message_cost(
        pricing: &PricingService,
        model_id: &str,
        provider_id: &str,
        tokens: CoreTokenBreakdown,
    ) -> f64 {
        pricing.calculate_cost_with_provider(model_id, Some(provider_id), &tokens)
    }

    fn assert_cost_matches(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected cost {expected}, got {actual}"
        );
    }

    fn make_workspace_message(
        client: &str,
        model_id: &str,
        provider_id: &str,
        session_id: &str,
        cost: f64,
        workspace_key: Option<&str>,
        workspace_label: Option<&str>,
    ) -> UnifiedMessage {
        let mut msg = UnifiedMessage::new(
            client,
            model_id,
            provider_id,
            session_id,
            1_735_689_600_000,
            tokscale_core::TokenBreakdown {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost,
        );
        msg.set_workspace(
            workspace_key.map(str::to_string),
            workspace_label.map(str::to_string),
        );
        msg
    }

    fn make_speed_message(
        session_id: &str,
        output_tokens: i64,
        reasoning_tokens: i64,
        duration_ms: u64,
    ) -> UnifiedMessage {
        let mut msg = UnifiedMessage::new(
            "codex",
            "gpt-5.4-high",
            "openai",
            session_id,
            1_735_689_600_000,
            tokscale_core::TokenBreakdown {
                input: 10,
                output: output_tokens,
                cache_read: 0,
                cache_write: 0,
                reasoning: reasoning_tokens,
            },
            0.0,
        );
        msg.generation_duration_ms = Some(duration_ms);
        msg
    }

    #[test]
    fn test_speed_rows_keep_only_reasonable_tps_samples() {
        let rows = build_speed_rows(&[
            make_speed_message("too-slow", 100, 0, 10_000),
            make_speed_message("reasonable-output", 100, 0, 2_000),
            make_speed_message("reasonable-reasoning", 50, 50, 1_000),
            make_speed_message("too-fast", 100, 0, 1),
        ]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model, "gpt-5.4");
        assert_eq!(rows[0].thinking_level, "High");
        assert_eq!(rows[0].generated_tokens, 200);
        assert_eq!(rows[0].generation_duration_ms, 3_000);
        assert_eq!(rows[0].sample_count, 2);
        assert!((rows[0].tokens_per_second() - 66.666_666_666_7).abs() < 1e-6);

        let summaries = build_speed_summaries(&rows);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].generated_tokens, 200);
        assert_eq!(summaries[0].sample_count, 2);
    }

    #[test]
    fn test_client_all() {
        let clients = ClientId::ALL;
        assert_eq!(clients.len(), 21);
        assert_eq!(clients[0], ClientId::OpenCode);
        assert_eq!(clients[1], ClientId::Claude);
        assert_eq!(clients[2], ClientId::Codex);
        assert_eq!(clients[3], ClientId::Cursor);
        assert_eq!(clients[4], ClientId::Gemini);
        assert_eq!(clients[5], ClientId::Amp);
        assert_eq!(clients[6], ClientId::Droid);
        assert_eq!(clients[7], ClientId::OpenClaw);
        assert_eq!(clients[8], ClientId::Pi);
        assert_eq!(clients[9], ClientId::Kimi);
        assert_eq!(clients[10], ClientId::Qwen);
        assert_eq!(clients[11], ClientId::RooCode);
        assert_eq!(clients[12], ClientId::KiloCode);
        assert_eq!(clients[13], ClientId::Mux);
        assert_eq!(clients[14], ClientId::Kilo);
        assert_eq!(clients[15], ClientId::Crush);
        assert_eq!(clients[16], ClientId::Hermes);
        assert_eq!(clients[17], ClientId::Copilot);
        assert_eq!(clients[18], ClientId::Goose);
        assert_eq!(clients[19], ClientId::Codebuff);
        assert_eq!(clients[20], ClientId::Antigravity);
    }

    #[test]
    fn test_client_as_str() {
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::OpenCode),
            "OpenCode"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Claude),
            "Claude"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Codex),
            "Codex"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Copilot),
            "Copilot"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Cursor),
            "Cursor"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Gemini),
            "Gemini"
        );
        assert_eq!(crate::tui::client_ui::display_name(ClientId::Amp), "Amp");
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Droid),
            "Droid"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::OpenClaw),
            "OpenClaw"
        );
        assert_eq!(crate::tui::client_ui::display_name(ClientId::Pi), "Pi");
        assert_eq!(crate::tui::client_ui::display_name(ClientId::Kimi), "Kimi");
        assert_eq!(crate::tui::client_ui::display_name(ClientId::Qwen), "Qwen");
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::RooCode),
            "Roo Code"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::KiloCode),
            "KiloCode"
        );
        assert_eq!(crate::tui::client_ui::display_name(ClientId::Mux), "Mux");
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Kilo),
            "Kilo CLI"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Crush),
            "Crush"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Hermes),
            "Hermes Agent"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Codebuff),
            "Codebuff"
        );
        assert_eq!(
            crate::tui::client_ui::display_name(ClientId::Antigravity),
            "Antigravity"
        );
    }

    #[test]
    fn test_client_key() {
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::OpenCode), '1');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Claude), '2');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Codex), '3');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Copilot), 'c');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Cursor), '4');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Gemini), '5');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Amp), '6');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Droid), '7');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::OpenClaw), '8');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Pi), '9');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Kimi), '0');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Qwen), 'w');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::RooCode), 'r');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::KiloCode), 'k');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Mux), 'x');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Kilo), 'l');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Crush), 'h');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Hermes), 'e');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Codebuff), 'b');
        assert_eq!(crate::tui::client_ui::hotkey(ClientId::Antigravity), 'a');
    }

    #[test]
    fn test_client_from_key() {
        assert_eq!(
            crate::tui::client_ui::from_hotkey('1'),
            Some(ClientId::OpenCode)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('2'),
            Some(ClientId::Claude)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('3'),
            Some(ClientId::Codex)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('c'),
            Some(ClientId::Copilot)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('4'),
            Some(ClientId::Cursor)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('5'),
            Some(ClientId::Gemini)
        );
        assert_eq!(crate::tui::client_ui::from_hotkey('6'), Some(ClientId::Amp));
        assert_eq!(
            crate::tui::client_ui::from_hotkey('7'),
            Some(ClientId::Droid)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('8'),
            Some(ClientId::OpenClaw)
        );
        assert_eq!(crate::tui::client_ui::from_hotkey('9'), Some(ClientId::Pi));
        assert_eq!(
            crate::tui::client_ui::from_hotkey('0'),
            Some(ClientId::Kimi)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('w'),
            Some(ClientId::Qwen)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('r'),
            Some(ClientId::RooCode)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('k'),
            Some(ClientId::KiloCode)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('l'),
            Some(ClientId::Kilo)
        );
        assert_eq!(crate::tui::client_ui::from_hotkey('x'), Some(ClientId::Mux));
        assert_eq!(
            crate::tui::client_ui::from_hotkey('h'),
            Some(ClientId::Crush)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('e'),
            Some(ClientId::Hermes)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('b'),
            Some(ClientId::Codebuff)
        );
        assert_eq!(
            crate::tui::client_ui::from_hotkey('a'),
            Some(ClientId::Antigravity)
        );
    }

    #[test]
    fn test_token_breakdown_total() {
        let breakdown = TokenBreakdown {
            input: 100,
            output: 200,
            cache_read: 50,
            cache_write: 25,
            reasoning: 10,
        };
        assert_eq!(breakdown.total(), 385);
    }

    #[test]
    fn test_token_breakdown_total_with_overflow() {
        let breakdown = TokenBreakdown {
            input: u64::MAX,
            output: 1,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        };
        // saturating_add should prevent overflow
        assert_eq!(breakdown.total(), u64::MAX);
    }

    #[test]
    fn test_token_breakdown_default() {
        let breakdown = TokenBreakdown::default();
        assert_eq!(breakdown.input, 0);
        assert_eq!(breakdown.output, 0);
        assert_eq!(breakdown.cache_read, 0);
        assert_eq!(breakdown.cache_write, 0);
        assert_eq!(breakdown.reasoning, 0);
        assert_eq!(breakdown.total(), 0);
    }

    #[test]
    fn test_data_loader_new() {
        let loader = DataLoader::new(None);
        assert!(loader._sessions_path.is_none());
        assert!(loader.since.is_none());
        assert!(loader.until.is_none());
        assert!(loader.year.is_none());
    }

    #[test]
    fn test_data_loader_scanner_settings_is_hermetic_under_cfg_test() {
        // Regression guard: the `#[cfg(test)]` branch of
        // `data_loader_scanner_settings` must not read
        // `~/.config/tokscale/settings.json`. Otherwise every DataLoader
        // unit test becomes machine-dependent as soon as a developer
        // pins extra OpenCode dbs in their real settings.json.
        //
        // This test cannot sandbox HOME (many of the sibling tests in
        // this module would race against each other if it did), so
        // instead it asserts the cfg(test) helper returns a default
        // ScannerSettings regardless of what the real settings file
        // contains on the developer's machine.
        let settings = super::data_loader_scanner_settings();
        assert!(
            settings.opencode_db_paths.is_empty(),
            "under #[cfg(test)] data_loader_scanner_settings must return \
             ScannerSettings::default() so unit tests stay hermetic, but \
             got {:?}",
            settings.opencode_db_paths
        );
    }

    #[test]
    fn test_data_loader_with_filters() {
        let loader = DataLoader::with_filters(
            Some(PathBuf::from("/tmp/sessions")),
            Some("2024-01-01".to_string()),
            Some("2024-12-31".to_string()),
            Some("2024".to_string()),
        );

        assert_eq!(loader._sessions_path, Some(PathBuf::from("/tmp/sessions")));
        assert_eq!(loader.since, Some("2024-01-01".to_string()));
        assert_eq!(loader.until, Some("2024-12-31".to_string()));
        assert_eq!(loader.year, Some("2024".to_string()));
    }

    #[test]
    fn test_parse_date() {
        assert_eq!(
            parse_date("2024-01-15"),
            Some(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap())
        );
        assert_eq!(
            parse_date("2024-12-31"),
            Some(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap())
        );
        assert_eq!(parse_date("invalid"), None);
        assert_eq!(parse_date("2024-13-01"), None);
        assert_eq!(parse_date(""), None);
    }

    #[test]
    fn test_build_contribution_graph_uses_provided_today() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let graph = build_contribution_graph_for_today(&[], today);
        assert!(graph.weeks.is_empty());

        let daily = vec![DailyUsage {
            date: NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            source_breakdown: BTreeMap::new(),
            message_count: 0,
            turn_count: 0,
        }];
        let graph = build_contribution_graph_for_today(&daily, today);
        let last_day = graph
            .weeks
            .last()
            .and_then(|week| week.last())
            .and_then(|day| day.as_ref())
            .map(|day| day.date);
        assert_eq!(last_day, Some(today));
    }

    #[test]
    fn test_aggregate_messages_builds_agent_usage() {
        let loader = DataLoader::new(None);
        let messages = vec![
            UnifiedMessage::new_with_agent(
                "opencode",
                "claude-sonnet-4",
                "anthropic",
                "session-1",
                1_735_689_600_000,
                tokscale_core::TokenBreakdown {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                1.25,
                Some("builder".to_string()),
            ),
            UnifiedMessage::new_with_agent(
                "roocode",
                "claude-sonnet-4",
                "anthropic",
                "session-2",
                1_735_689_700_000,
                tokscale_core::TokenBreakdown {
                    input: 20,
                    output: 10,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                2.75,
                Some("builder".to_string()),
            ),
        ];

        let usage = loader
            .aggregate_messages(messages, &GroupBy::Model)
            .unwrap();

        assert_eq!(usage.agents.len(), 1);
        assert_eq!(usage.agents[0].agent, "Builder");
        assert_eq!(usage.agents[0].clients, "opencode, roocode");
        assert_eq!(usage.agents[0].message_count, 2);
        assert!((usage.agents[0].cost - 4.0).abs() < f64::EPSILON);
        assert_eq!(usage.agents[0].tokens.total(), 45);
    }

    #[test]
    fn test_aggregate_messages_builds_codex_account_usage() {
        let loader = DataLoader::new(None);
        let mut attributed = UnifiedMessage::new(
            "codex",
            "gpt-5",
            "openai",
            "session-1",
            1_735_689_600_000,
            tokscale_core::TokenBreakdown {
                input: 10,
                output: 5,
                cache_read: 1,
                cache_write: 0,
                reasoning: 2,
            },
            1.5,
        );
        attributed.codex_account_hash = Some("abc123def456".to_string());
        attributed.is_turn_start = true;

        let mut same_session = UnifiedMessage::new(
            "codex",
            "gpt-5",
            "openai",
            "session-1",
            1_735_689_700_000,
            tokscale_core::TokenBreakdown {
                input: 4,
                output: 3,
                cache_read: 0,
                cache_write: 0,
                reasoning: 1,
            },
            0.5,
        );
        same_session.codex_account_hash = Some("abc123def456".to_string());

        let unattributed = UnifiedMessage::new(
            "codex",
            "gpt-5",
            "openai",
            "session-2",
            1_735_689_800_000,
            tokscale_core::TokenBreakdown {
                input: 2,
                output: 1,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            0.25,
        );

        let usage = loader
            .aggregate_messages(
                vec![attributed, same_session, unattributed],
                &GroupBy::Model,
            )
            .unwrap();

        assert_eq!(usage.codex_accounts.len(), 2);
        let account = usage
            .codex_accounts
            .iter()
            .find(|row| row.account_hash == "abc123def456")
            .unwrap();
        assert_eq!(account.tokens.total(), 26);
        assert_eq!(account.session_count, 1);
        assert_eq!(account.turn_count, 1);
        assert_eq!(account.message_count, 2);
        assert_eq!(account.active_month_count, Some(1));
        assert!((account.cost - 2.0).abs() < f64::EPSILON);
        assert_eq!(account.paid_cost, Some(200.0));

        let unknown = usage
            .codex_accounts
            .iter()
            .find(|row| row.account_hash == "unattributed")
            .unwrap();
        assert_eq!(unknown.session_count, 1);
        assert_eq!(unknown.active_month_count, None);
        assert_eq!(unknown.paid_cost, None);
        assert_eq!(unknown.tokens.total(), 3);
    }

    #[test]
    fn test_aggregate_messages_builds_price_rows_from_pricing() {
        let loader = DataLoader::new(None);
        let pricing = test_pricing_service();
        let messages = vec![
            UnifiedMessage::new(
                "codex",
                "claude-sonnet-4",
                "anthropic",
                "session-1",
                1_735_689_600_000,
                tokscale_core::TokenBreakdown {
                    input: 100,
                    output: 20,
                    cache_read: 5,
                    cache_write: 0,
                    reasoning: 0,
                },
                0.0,
            ),
            UnifiedMessage::new(
                "cursor",
                "claude-sonnet-4",
                "anthropic",
                "session-2",
                1_735_689_700_000,
                tokscale_core::TokenBreakdown {
                    input: 40,
                    output: 10,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                0.0,
            ),
        ];

        let usage = loader
            .aggregate_messages_with_pricing(messages, Vec::new(), &GroupBy::Model, Some(&pricing))
            .unwrap();

        assert_eq!(usage.prices.len(), 1);
        let price_row = &usage.prices[0];
        assert_eq!(price_row.model, "claude-sonnet-4");
        assert_eq!(price_row.provider, "anthropic");
        assert_eq!(price_row.pricing_source.as_deref(), Some("LiteLLM"));
        assert_eq!(price_row.input_price_per_million, Some(10.0));
        assert_eq!(price_row.output_price_per_million, Some(20.0));
        assert_eq!(price_row.cache_read_price_per_million, Some(3.0));
        assert_eq!(price_row.tokens.input, 140);
        assert_eq!(price_row.tokens.output, 30);
        assert_eq!(price_row.clients.len(), 2);
    }

    #[test]
    fn test_quota_value_intervals_join_codex_cost_between_samples() {
        let messages = vec![UnifiedMessage::new(
            "codex",
            "gpt-5.4",
            "openai",
            "session-1",
            1_735_691_400_000,
            tokscale_core::TokenBreakdown {
                input: 100,
                output: 25,
                cache_read: 5,
                cache_write: 0,
                reasoning: 10,
            },
            3.50,
        )];
        let samples = vec![
            CodexQuotaSample::new(
                "session-1",
                1_735_689_600_000,
                "openai",
                "gpt-5.4",
                "secondary",
                10.0,
                10080,
                1_736_294_400_000,
            ),
            CodexQuotaSample::new(
                "session-1",
                1_735_693_200_000,
                "openai",
                "gpt-5.4",
                "secondary",
                15.0,
                10080,
                1_736_294_400_000,
            ),
        ];

        let quota = build_quota_value_data(&messages, &samples);

        assert_eq!(quota.sample_count, 2);
        assert_eq!(quota.intervals.len(), 1);
        let interval = &quota.intervals[0];
        assert_eq!(interval.window_kind, "secondary");
        assert_eq!(interval.model, "gpt-5.4");
        assert_eq!(interval.quota_burn_pct, 5.0);
        assert_eq!(interval.api_value_usd, 3.50);
        assert_eq!(interval.tokens.total(), 140);
        assert!(interval.models.contains("gpt-5.4"));
        assert!(interval.subscription_cost_burned > 0.0);
        assert!(interval.factor.unwrap() > 0.0);
        assert_eq!(quota.points.len(), 1);
        assert_eq!(quota.points[0].model, "gpt-5.4");
        assert_eq!(quota.points[0].interval_count, 1);
        assert_eq!(quota.model_summaries.len(), 1);
        assert_eq!(quota.model_summaries[0].model, "gpt-5.4");
        assert_eq!(quota.model_summaries[0].tokens.total(), 140);
    }

    #[test]
    fn test_quota_value_intervals_split_mixed_windows_by_model_tokens() {
        let messages = vec![
            UnifiedMessage::new(
                "codex",
                "gpt-5.4",
                "openai",
                "session-1",
                1_735_691_400_000,
                tokscale_core::TokenBreakdown {
                    input: 75,
                    output: 25,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                6.0,
            ),
            UnifiedMessage::new(
                "codex",
                "gpt-5.4-mini",
                "openai",
                "session-1",
                1_735_691_500_000,
                tokscale_core::TokenBreakdown {
                    input: 25,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                1.0,
            ),
        ];
        let samples = vec![
            CodexQuotaSample::new(
                "session-1",
                1_735_689_600_000,
                "openai",
                "gpt-5.4",
                "secondary",
                10.0,
                10080,
                1_736_294_400_000,
            ),
            CodexQuotaSample::new(
                "session-1",
                1_735_693_200_000,
                "openai",
                "gpt-5.4",
                "secondary",
                15.0,
                10080,
                1_736_294_400_000,
            ),
        ];

        let quota = build_quota_value_data(&messages, &samples);

        assert_eq!(quota.intervals.len(), 2);
        let gpt = quota
            .intervals
            .iter()
            .find(|interval| interval.model == "gpt-5.4")
            .unwrap();
        let mini = quota
            .intervals
            .iter()
            .find(|interval| interval.model == "gpt-5.4-mini")
            .unwrap();
        assert!((gpt.quota_burn_pct - 4.0).abs() < 1e-9);
        assert!((mini.quota_burn_pct - 1.0).abs() < 1e-9);
        assert_eq!(gpt.tokens.total(), 100);
        assert_eq!(mini.tokens.total(), 25);
        assert_eq!(quota.points.len(), 2);
        assert_eq!(quota.model_summaries.len(), 2);
    }

    #[test]
    fn test_quota_value_skips_zero_and_negative_burn_intervals() {
        let samples = vec![
            CodexQuotaSample::new(
                "session-1",
                1_735_689_600_000,
                "openai",
                "gpt-5.4",
                "secondary",
                10.0,
                10080,
                1_736_294_400_000,
            ),
            CodexQuotaSample::new(
                "session-1",
                1_735_693_200_000,
                "openai",
                "gpt-5.4",
                "secondary",
                10.0,
                10080,
                1_736_294_400_000,
            ),
            CodexQuotaSample::new(
                "session-1",
                1_735_696_800_000,
                "openai",
                "gpt-5.4",
                "secondary",
                9.0,
                10080,
                1_736_294_400_000,
            ),
        ];

        let quota = build_quota_value_data(&[], &samples);

        assert_eq!(quota.sample_count, 3);
        assert!(quota.intervals.is_empty());
        assert!(quota.points.is_empty());
    }

    #[test]
    fn test_quota_value_rolling_points_use_weighted_ratio() {
        let samples = vec![
            CodexQuotaSample::new(
                "session-1",
                1_735_689_600_000,
                "openai",
                "gpt-5.4",
                "secondary",
                0.0,
                10080,
                1_736_294_400_000,
            ),
            CodexQuotaSample::new(
                "session-1",
                1_735_693_200_000,
                "openai",
                "gpt-5.4",
                "secondary",
                1.0,
                10080,
                1_736_294_400_000,
            ),
            CodexQuotaSample::new(
                "session-1",
                1_735_776_000_000,
                "openai",
                "gpt-5.4",
                "secondary",
                3.0,
                10080,
                1_736_294_400_000,
            ),
        ];
        let messages = vec![
            UnifiedMessage::new(
                "codex",
                "gpt-5.4",
                "openai",
                "session-1",
                1_735_691_400_000,
                tokscale_core::TokenBreakdown {
                    input: 10,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                1.0,
            ),
            UnifiedMessage::new(
                "codex",
                "gpt-5.4",
                "openai",
                "session-1",
                1_735_734_000_000,
                tokscale_core::TokenBreakdown {
                    input: 20,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                9.0,
            ),
        ];

        let quota = build_quota_value_data(&messages, &samples);

        assert_eq!(quota.intervals.len(), 2);
        let latest = quota.points.last().unwrap();
        let expected_api: f64 = quota
            .intervals
            .iter()
            .map(|interval| interval.api_value_usd)
            .sum();
        let expected_burned: f64 = quota
            .intervals
            .iter()
            .map(|interval| interval.subscription_cost_burned)
            .sum();
        let expected_factor = expected_api / expected_burned;
        let naive_factor_average = quota
            .intervals
            .iter()
            .filter_map(|interval| interval.factor)
            .sum::<f64>()
            / quota.intervals.len() as f64;

        assert_eq!(latest.interval_count, 2);
        assert!((latest.factor.unwrap() - expected_factor).abs() < 1e-9);
        assert!((latest.factor.unwrap() - naive_factor_average).abs() > 1e-6);
    }

    #[test]
    fn test_aggregate_messages_dedupes_price_rows_by_canonical_model() {
        let loader = DataLoader::new(None);
        let messages = vec![
            UnifiedMessage::new(
                "codex",
                "claude-sonnet-4-20250514",
                "anthropic",
                "session-1",
                1_735_689_600_000,
                tokscale_core::TokenBreakdown {
                    input: 100,
                    output: 20,
                    cache_read: 5,
                    cache_write: 0,
                    reasoning: 0,
                },
                1.0,
            ),
            UnifiedMessage::new(
                "cursor",
                "claude-sonnet-4-20250601",
                "anthropic",
                "session-2",
                1_735_689_700_000,
                tokscale_core::TokenBreakdown {
                    input: 40,
                    output: 10,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                2.0,
            ),
        ];

        let usage = loader
            .aggregate_messages(messages, &GroupBy::Model)
            .unwrap();

        assert_eq!(usage.prices.len(), 1);
        let price_row = &usage.prices[0];
        assert_eq!(price_row.model, "claude-sonnet-4");
        assert_eq!(price_row.matched_key.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(price_row.provider, "anthropic");
        assert_eq!(price_row.tokens.input, 140);
        assert_eq!(price_row.tokens.output, 30);
        assert!((price_row.cost - 3.0).abs() < f64::EPSILON);
        assert_eq!(price_row.clients.len(), 2);
    }

    #[test]
    fn test_aggregate_messages_groups_by_workspace_and_model() {
        let loader = DataLoader::new(None);
        let usage = loader
            .aggregate_messages(
                vec![
                    make_workspace_message(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-1",
                        1.25,
                        Some("/repo-a"),
                        Some("repo-a"),
                    ),
                    make_workspace_message(
                        "qwen",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-2",
                        2.75,
                        Some("/repo-a"),
                        Some("repo-a"),
                    ),
                ],
                &GroupBy::WorkspaceModel,
            )
            .unwrap();

        assert_eq!(usage.models.len(), 1);
        assert_eq!(usage.models[0].workspace_key.as_deref(), Some("/repo-a"));
        assert_eq!(usage.models[0].workspace_label.as_deref(), Some("repo-a"));
        assert_eq!(usage.models[0].model, "claude-sonnet-4-5");
        assert_eq!(usage.models[0].client, "claude, qwen");
        assert_eq!(usage.models[0].session_count, 2);
        assert_eq!(usage.models[0].cost, 4.0);
    }

    #[test]
    fn test_aggregate_messages_workspace_grouping_keeps_unknown_bucket_visible() {
        let loader = DataLoader::new(None);
        let usage = loader
            .aggregate_messages(
                vec![
                    make_workspace_message(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-1",
                        1.0,
                        None,
                        None,
                    ),
                    make_workspace_message(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-2",
                        2.0,
                        None,
                        None,
                    ),
                ],
                &GroupBy::WorkspaceModel,
            )
            .unwrap();

        assert_eq!(usage.models.len(), 1);
        assert_eq!(usage.models[0].workspace_key, None);
        assert_eq!(
            usage.models[0].workspace_label.as_deref(),
            Some(UNKNOWN_WORKSPACE_LABEL)
        );
        assert_eq!(usage.models[0].session_count, 2);
        assert_eq!(usage.models[0].cost, 3.0);
    }

    #[test]
    fn test_aggregate_messages_workspace_grouping_keeps_real_unknown_workspace_separate() {
        let loader = DataLoader::new(None);
        let usage = loader
            .aggregate_messages(
                vec![
                    make_workspace_message(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-1",
                        1.0,
                        Some("unknown-workspace"),
                        Some("unknown-workspace"),
                    ),
                    make_workspace_message(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-2",
                        2.0,
                        None,
                        None,
                    ),
                ],
                &GroupBy::WorkspaceModel,
            )
            .unwrap();

        assert_eq!(usage.models.len(), 2);
        assert!(usage.models.iter().any(|model| {
            model.workspace_key.as_deref() == Some("unknown-workspace")
                && model.workspace_label.as_deref() == Some("unknown-workspace")
                && (model.cost - 1.0).abs() < f64::EPSILON
        }));
        assert!(usage.models.iter().any(|model| {
            model.workspace_key.is_none()
                && model.workspace_label.as_deref() == Some(UNKNOWN_WORKSPACE_LABEL)
                && (model.cost - 2.0).abs() < f64::EPSILON
        }));
    }

    #[test]
    fn test_aggregate_messages_workspace_grouping_splits_daily_models_by_workspace() {
        let loader = DataLoader::new(None);
        let usage = loader
            .aggregate_messages(
                vec![
                    make_workspace_message(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-1",
                        1.0,
                        Some("/repo-a"),
                        Some("repo-a"),
                    ),
                    make_workspace_message(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-2",
                        2.0,
                        Some("/repo-b"),
                        Some("repo-b"),
                    ),
                ],
                &GroupBy::WorkspaceModel,
            )
            .unwrap();

        assert_eq!(usage.daily.len(), 1);
        let claude = usage.daily[0].source_breakdown.get("claude").unwrap();
        let daily_keys: Vec<_> = claude.models.keys().cloned().collect();
        assert_eq!(daily_keys.len(), 2);
        assert_ne!(daily_keys[0], daily_keys[1]);
        let daily_display_names: Vec<_> = claude
            .models
            .values()
            .map(|info| info.display_name.clone())
            .collect();
        assert_eq!(
            daily_display_names,
            vec![
                "repo-a / claude-sonnet-4-5".to_string(),
                "repo-b / claude-sonnet-4-5".to_string()
            ]
        );
    }

    #[test]
    fn test_aggregate_messages_workspace_grouping_disambiguates_identical_labels() {
        let loader = DataLoader::new(None);
        let usage = loader
            .aggregate_messages(
                vec![
                    make_workspace_message(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-1",
                        1.0,
                        Some("/srv/team-a/demo"),
                        Some("demo"),
                    ),
                    make_workspace_message(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-2",
                        2.0,
                        Some("/srv/team-b/demo"),
                        Some("demo"),
                    ),
                ],
                &GroupBy::WorkspaceModel,
            )
            .unwrap();

        assert_eq!(usage.daily.len(), 1);
        let claude = usage.daily[0].source_breakdown.get("claude").unwrap();
        assert_eq!(claude.models.len(), 2);

        // Keys must differ even though display names are identical
        let daily_keys: Vec<_> = claude.models.keys().cloned().collect();
        assert_eq!(daily_keys.len(), 2);
        assert_ne!(daily_keys[0], daily_keys[1]);

        let display_names: Vec<_> = claude
            .models
            .values()
            .map(|info| info.display_name.clone())
            .collect();
        assert_eq!(
            display_names,
            vec![
                "demo / claude-sonnet-4-5".to_string(),
                "demo / claude-sonnet-4-5".to_string()
            ]
        );
    }

    #[test]
    fn test_aggregate_messages_client_provider_model_splits_providers_in_daily_breakdown() {
        let loader = DataLoader::new(None);
        let usage = loader
            .aggregate_messages(
                vec![
                    UnifiedMessage::new(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-1",
                        1_735_689_600_000,
                        tokscale_core::TokenBreakdown {
                            input: 10,
                            output: 5,
                            cache_read: 0,
                            cache_write: 0,
                            reasoning: 0,
                        },
                        1.0,
                    ),
                    UnifiedMessage::new(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "github-copilot",
                        "session-2",
                        1_735_689_600_000,
                        tokscale_core::TokenBreakdown {
                            input: 20,
                            output: 10,
                            cache_read: 0,
                            cache_write: 0,
                            reasoning: 0,
                        },
                        2.0,
                    ),
                ],
                &GroupBy::ClientProviderModel,
            )
            .unwrap();

        assert_eq!(usage.daily.len(), 1);
        let claude = usage.daily[0].source_breakdown.get("claude").unwrap();
        assert_eq!(claude.models.len(), 2);

        let anthropic_key = "anthropic:claude-sonnet-4-5";
        let copilot_key = "github-copilot:claude-sonnet-4-5";
        let anthropic_model = claude.models.get(anthropic_key).unwrap();
        assert_eq!(
            anthropic_model.display_name,
            "anthropic / claude-sonnet-4-5"
        );
        assert_eq!(anthropic_model.provider, "anthropic");
        assert_eq!(anthropic_model.tokens.total(), 15);
        assert_eq!(anthropic_model.messages, 1);

        let copilot_model = claude.models.get(copilot_key).unwrap();
        assert_eq!(
            copilot_model.display_name,
            "github-copilot / claude-sonnet-4-5"
        );
        assert_eq!(copilot_model.provider, "github-copilot");
        assert_eq!(copilot_model.tokens.total(), 30);
        assert_eq!(copilot_model.messages, 1);
    }

    #[test]
    fn test_aggregate_messages_keeps_same_model_split_across_sources_in_daily_breakdown() {
        let loader = DataLoader::new(None);
        let usage = loader
            .aggregate_messages(
                vec![
                    UnifiedMessage::new(
                        "claude",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-1",
                        1_735_689_600_000,
                        tokscale_core::TokenBreakdown {
                            input: 10,
                            output: 5,
                            cache_read: 0,
                            cache_write: 0,
                            reasoning: 0,
                        },
                        1.0,
                    ),
                    UnifiedMessage::new(
                        "cursor",
                        "claude-sonnet-4-5-20250929",
                        "anthropic",
                        "session-2",
                        1_735_689_600_000,
                        tokscale_core::TokenBreakdown {
                            input: 20,
                            output: 10,
                            cache_read: 0,
                            cache_write: 0,
                            reasoning: 0,
                        },
                        2.0,
                    ),
                ],
                &GroupBy::Model,
            )
            .unwrap();

        assert_eq!(usage.daily.len(), 1);
        assert_eq!(usage.daily[0].source_breakdown.len(), 2);

        let claude = usage.daily[0].source_breakdown.get("claude").unwrap();
        assert_eq!(claude.cost, 1.0);
        assert_eq!(claude.models.len(), 1);
        let claude_model = claude.models.get("claude-sonnet-4-5").unwrap();
        assert_eq!(claude_model.display_name, "claude-sonnet-4-5");
        assert_eq!(claude_model.tokens.total(), 15);

        let cursor = usage.daily[0].source_breakdown.get("cursor").unwrap();
        assert_eq!(cursor.cost, 2.0);
        assert_eq!(cursor.models.len(), 1);
        let cursor_model = cursor.models.get("claude-sonnet-4-5").unwrap();
        assert_eq!(cursor_model.display_name, "claude-sonnet-4-5");
        assert_eq!(cursor_model.tokens.total(), 30);
    }

    #[test]
    fn test_aggregate_messages_merges_oh_my_opencode_agent_variants() {
        let loader = DataLoader::new(None);
        let messages = vec![
            UnifiedMessage::new_with_agent(
                "opencode",
                "claude-opus-4-6",
                "anthropic",
                "session-1",
                1_735_689_600_000,
                tokscale_core::TokenBreakdown {
                    input: 10,
                    output: 5,
                    cache_read: 100,
                    cache_write: 20,
                    reasoning: 0,
                },
                1.5,
                Some("Sisyphus".to_string()),
            ),
            UnifiedMessage::new_with_agent(
                "opencode",
                "claude-opus-4-6",
                "anthropic",
                "session-2",
                1_735_689_700_000,
                tokscale_core::TokenBreakdown {
                    input: 20,
                    output: 10,
                    cache_read: 200,
                    cache_write: 40,
                    reasoning: 0,
                },
                2.5,
                Some("Sisyphus (Ultraworker)".to_string()),
            ),
        ];

        let usage = loader
            .aggregate_messages(messages, &GroupBy::Model)
            .unwrap();

        assert_eq!(usage.agents.len(), 1);
        assert_eq!(usage.agents[0].agent, "Sisyphus");
        assert_eq!(usage.agents[0].clients, "opencode");
        assert_eq!(usage.agents[0].message_count, 2);
        assert!((usage.agents[0].cost - 4.0).abs() < f64::EPSILON);
        assert_eq!(usage.agents[0].tokens.total(), 405);
    }

    #[test]
    fn test_aggregate_messages_merges_opencode_agent_case_variants() {
        let loader = DataLoader::new(None);
        let messages = vec![
            UnifiedMessage::new_with_agent(
                "opencode",
                "claude-opus-4-6",
                "anthropic",
                "session-1",
                1_735_689_600_000,
                tokscale_core::TokenBreakdown {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                1.5,
                Some("Hephaestus".to_string()),
            ),
            UnifiedMessage::new_with_agent(
                "opencode",
                "claude-opus-4-6",
                "anthropic",
                "session-2",
                1_735_689_700_000,
                tokscale_core::TokenBreakdown {
                    input: 20,
                    output: 10,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                2.5,
                Some("hephaestus".to_string()),
            ),
        ];

        let usage = loader
            .aggregate_messages(messages, &GroupBy::Model)
            .unwrap();

        assert_eq!(usage.agents.len(), 1);
        assert_eq!(usage.agents[0].agent, "Hephaestus");
        assert_eq!(usage.agents[0].clients, "opencode");
        assert_eq!(usage.agents[0].message_count, 2);
        assert!((usage.agents[0].cost - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_aggregate_messages_does_not_merge_omo_variants_for_non_opencode_clients() {
        let loader = DataLoader::new(None);
        let messages = vec![
            UnifiedMessage::new_with_agent(
                "claude",
                "claude-opus-4-6",
                "anthropic",
                "session-1",
                1_735_689_600_000,
                tokscale_core::TokenBreakdown {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                1.5,
                Some("Sisyphus".to_string()),
            ),
            UnifiedMessage::new_with_agent(
                "claude",
                "claude-opus-4-6",
                "anthropic",
                "session-2",
                1_735_689_700_000,
                tokscale_core::TokenBreakdown {
                    input: 20,
                    output: 10,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
                2.5,
                Some("Sisyphus (Ultraworker)".to_string()),
            ),
        ];

        let usage = loader
            .aggregate_messages(messages, &GroupBy::Model)
            .unwrap();

        assert_eq!(usage.agents.len(), 2);
        assert!(usage.agents.iter().any(|agent| agent.agent == "Sisyphus"));
        assert!(usage
            .agents
            .iter()
            .any(|agent| agent.agent == "Sisyphus (Ultraworker)"));
    }

    #[test]
    #[serial]
    fn test_data_loader_loads_agent_usage_from_roocode_files() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let task_root = temp_dir
            .path()
            .join(".config/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks");

        let architect_dir = task_root.join("task-architect");
        fs::create_dir_all(&architect_dir).unwrap();
        fs::write(
            architect_dir.join("ui_messages.json"),
            r#"[
  {
    "type": "say",
    "say": "api_req_started",
    "ts": "2026-03-07T16:00:00Z",
    "text": "{\"cost\":8.4,\"tokensIn\":420000,\"tokensOut\":120000,\"cacheReads\":32000,\"cacheWrites\":0,\"apiProtocol\":\"anthropic\"}"
  },
  {
    "type": "say",
    "say": "api_req_started",
    "ts": "2026-03-07T16:05:00Z",
    "text": "{\"cost\":3.1,\"tokensIn\":90000,\"tokensOut\":60000,\"cacheReads\":12000,\"cacheWrites\":0,\"apiProtocol\":\"anthropic\"}"
  }
]"#,
        )
        .unwrap();
        fs::write(
            architect_dir.join("api_conversation_history.json"),
            r#"before
<environment_details>
<model>claude-sonnet-4</model>
<slug>architect</slug>
<name>Architect</name>
</environment_details>
after"#,
        )
        .unwrap();

        let reviewer_dir = task_root.join("task-reviewer");
        fs::create_dir_all(&reviewer_dir).unwrap();
        fs::write(
            reviewer_dir.join("ui_messages.json"),
            r#"[
  {
    "type": "say",
    "say": "api_req_started",
    "ts": "2026-03-07T17:00:00Z",
    "text": "{\"cost\":1.8,\"tokensIn\":70000,\"tokensOut\":26000,\"cacheReads\":8000,\"cacheWrites\":0,\"apiProtocol\":\"anthropic\"}"
  },
  {
    "type": "say",
    "say": "api_req_started",
    "ts": "2026-03-07T17:09:00Z",
    "text": "{\"cost\":0.9,\"tokensIn\":22000,\"tokensOut\":18000,\"cacheReads\":3000,\"cacheWrites\":0,\"apiProtocol\":\"anthropic\"}"
  }
]"#,
        )
        .unwrap();
        fs::write(
            reviewer_dir.join("api_conversation_history.json"),
            r#"before
<environment_details>
<model>claude-haiku-4</model>
<slug>reviewer</slug>
<name>Reviewer</name>
</environment_details>
after"#,
        )
        .unwrap();

        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let pricing = test_pricing_service();
        let loader = DataLoader::new(None);
        let usage = load_with_pricing(
            &loader,
            &[ClientId::RooCode],
            &GroupBy::Model,
            false,
            Some(&pricing),
        )
        .unwrap();

        let architect_expected = expected_message_cost(
            &pricing,
            "claude-sonnet-4",
            "anthropic",
            CoreTokenBreakdown {
                input: 420_000,
                output: 120_000,
                cache_read: 32_000,
                cache_write: 0,
                reasoning: 0,
            },
        ) + expected_message_cost(
            &pricing,
            "claude-sonnet-4",
            "anthropic",
            CoreTokenBreakdown {
                input: 90_000,
                output: 60_000,
                cache_read: 12_000,
                cache_write: 0,
                reasoning: 0,
            },
        );
        let reviewer_expected = expected_message_cost(
            &pricing,
            "claude-haiku-4",
            "anthropic",
            CoreTokenBreakdown {
                input: 70_000,
                output: 26_000,
                cache_read: 8_000,
                cache_write: 0,
                reasoning: 0,
            },
        ) + expected_message_cost(
            &pricing,
            "claude-haiku-4",
            "anthropic",
            CoreTokenBreakdown {
                input: 22_000,
                output: 18_000,
                cache_read: 3_000,
                cache_write: 0,
                reasoning: 0,
            },
        );

        assert_eq!(usage.agents.len(), 2);
        assert_eq!(usage.agents[0].agent, "Architect");
        assert_eq!(usage.agents[0].clients, "roocode");
        assert_eq!(usage.agents[0].message_count, 2);
        assert_cost_matches(usage.agents[0].cost, architect_expected);
        assert_eq!(usage.agents[0].tokens.total(), 734_000);

        assert_eq!(usage.agents[1].agent, "Reviewer");
        assert_eq!(usage.agents[1].message_count, 2);
        assert_cost_matches(usage.agents[1].cost, reviewer_expected);
        assert_eq!(usage.agents[1].tokens.total(), 147_000);

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    #[serial]
    fn test_data_loader_keeps_synthetic_gateway_messages_under_original_client() {
        let temp_dir = TempDir::new().unwrap();
        let previous_home = env::var_os("HOME");
        let message_dir = temp_dir
            .path()
            .join(".local/share/opencode/storage/message/project-1");
        fs::create_dir_all(&message_dir).unwrap();
        fs::write(
            message_dir.join("msg_001.json"),
            r#"{"id":"msg-1","sessionID":"session-1","role":"assistant","modelID":"accounts/fireworks/models/deepseek-v3-0324","providerID":"fireworks","cost":0.25,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1733011200000}}"#,
        )
        .unwrap();

        unsafe {
            env::set_var("HOME", temp_dir.path());
        }

        let pricing = test_pricing_service();
        let loader = DataLoader::new(None);
        let usage = load_with_pricing(
            &loader,
            &[ClientId::OpenCode],
            &GroupBy::ClientProviderModel,
            true,
            Some(&pricing),
        )
        .unwrap();

        let expected_cost = expected_message_cost(
            &pricing,
            "accounts/fireworks/models/deepseek-v3-0324",
            "fireworks",
            CoreTokenBreakdown {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
        );

        assert_eq!(usage.models.len(), 1);
        assert_eq!(usage.models[0].client, "opencode");
        assert_eq!(usage.models[0].provider, "fireworks");
        assert_eq!(usage.models[0].model, "deepseek-v3-0324");
        assert_eq!(usage.models[0].tokens.total(), 15);
        assert_cost_matches(usage.models[0].cost, expected_cost);

        match previous_home {
            Some(home) => unsafe { env::set_var("HOME", home) },
            None => unsafe { env::remove_var("HOME") },
        }
    }

    #[test]
    fn test_calculate_streaks_uses_provided_today() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 3).unwrap();
        let daily = vec![
            DailyUsage {
                date: NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                source_breakdown: BTreeMap::new(),
                message_count: 0,
                turn_count: 0,
            },
            DailyUsage {
                date: NaiveDate::from_ymd_opt(2026, 3, 3).unwrap(),
                tokens: TokenBreakdown::default(),
                cost: 0.0,
                source_breakdown: BTreeMap::new(),
                message_count: 0,
                turn_count: 0,
            },
        ];
        let (current, longest) = calculate_streaks_for_today(&daily, today);
        assert_eq!(current, 2);
        assert_eq!(longest, 2);
    }
}
