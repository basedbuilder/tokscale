use anyhow::Result;
use serde_json::json;

use super::data::UsageData;

/// Serializes `UsageData` into the pretty-printed JSON payload used by the
/// `e` export hotkey. Pure: callers are responsible for file I/O and any
/// user-facing status messages.
pub fn build_export_json(data: &UsageData) -> Result<String> {
    let export_data = json!({
        "models": data.models.iter().map(|m| json!({
            "model": m.model,
            "provider": m.provider,
            "client": m.client,
            "tokens": {
                "input": m.tokens.input,
                "output": m.tokens.output,
                "cacheRead": m.tokens.cache_read,
                "cacheWrite": m.tokens.cache_write,
                "total": m.tokens.total()
            },
            "cost": m.cost,
            "sessionCount": m.session_count
        })).collect::<Vec<_>>(),
        "agents": data.agents.iter().map(|a| json!({
            "agent": a.agent,
            "clients": a.clients,
            "tokens": {
                "input": a.tokens.input,
                "output": a.tokens.output,
                "cacheRead": a.tokens.cache_read,
                "cacheWrite": a.tokens.cache_write,
                "total": a.tokens.total()
            },
            "cost": a.cost,
            "messageCount": a.message_count
        })).collect::<Vec<_>>(),
        "daily": data.daily.iter().map(|d| json!({
            "date": d.date.to_string(),
            "tokens": {
                "input": d.tokens.input,
                "output": d.tokens.output,
                "cacheRead": d.tokens.cache_read,
                "cacheWrite": d.tokens.cache_write,
                "total": d.tokens.total()
            },
            "messageCount": d.message_count,
            "turnCount": d.turn_count,
            "cost": d.cost
        })).collect::<Vec<_>>(),
        "prices": data.prices.iter().map(|p| json!({
            "model": p.model,
            "provider": p.provider,
            "pricingSource": p.pricing_source,
            "matchedKey": p.matched_key,
            "latestDate": p.latest_date.to_string(),
            "tokens": {
                "input": p.tokens.input,
                "output": p.tokens.output,
                "cacheRead": p.tokens.cache_read,
                "cacheWrite": p.tokens.cache_write,
                "total": p.tokens.total()
            },
            "cost": p.cost,
            "messageCount": p.message_count,
            "inputPricePerMillion": p.input_price_per_million,
            "outputPricePerMillion": p.output_price_per_million,
            "cacheReadPricePerMillion": p.cache_read_price_per_million,
            "cacheWritePricePerMillion": p.cache_write_price_per_million
        })).collect::<Vec<_>>(),
        "pricesDaily": data.prices_daily.iter().map(|p| json!({
            "date": p.date.to_string(),
            "model": p.model,
            "provider": p.provider,
            "pricingSource": p.pricing_source,
            "matchedKey": p.matched_key,
            "tokens": {
                "input": p.tokens.input,
                "output": p.tokens.output,
                "cacheRead": p.tokens.cache_read,
                "cacheWrite": p.tokens.cache_write,
                "total": p.tokens.total()
            },
            "cost": p.cost,
            "messageCount": p.message_count,
            "inputPricePerMillion": p.input_price_per_million,
            "outputPricePerMillion": p.output_price_per_million,
            "cacheReadPricePerMillion": p.cache_read_price_per_million,
            "cacheWritePricePerMillion": p.cache_write_price_per_million
        })).collect::<Vec<_>>(),
        "thinking": data.thinking.iter().map(|t| json!({
            "model": t.model,
            "tokens": {
                "input": t.tokens.input,
                "output": t.tokens.output,
                "cacheRead": t.tokens.cache_read,
                "cacheWrite": t.tokens.cache_write,
                "reasoning": t.tokens.reasoning,
                "total": t.tokens.total()
            },
            "cost": t.cost,
            "messageCount": t.message_count,
            "thirtyDayTrendPct": t.thirty_day_trend_pct
        })).collect::<Vec<_>>(),
        "thinkingDaily": data.thinking_daily.iter().map(|t| json!({
            "date": t.date.to_string(),
            "model": t.model,
            "provider": t.provider,
            "thinkingLevel": t.thinking_level,
            "tokens": {
                "input": t.tokens.input,
                "output": t.tokens.output,
                "cacheRead": t.tokens.cache_read,
                "cacheWrite": t.tokens.cache_write,
                "reasoning": t.tokens.reasoning,
                "total": t.tokens.total()
            },
            "cost": t.cost,
            "messageCount": t.message_count
        })).collect::<Vec<_>>(),
        "speeds": data.speeds.iter().map(|s| json!({
            "model": s.model,
            "provider": s.provider,
            "thinkingLevel": s.thinking_level,
            "latestDate": s.latest_date.to_string(),
            "tokensPerSecond": s.tokens_per_second(),
            "generatedTokens": s.generated_tokens,
            "generationDurationMs": s.generation_duration_ms,
            "sampleCount": s.sample_count
        })).collect::<Vec<_>>(),
        "speedsDaily": data.speeds_daily.iter().map(|s| json!({
            "date": s.date.to_string(),
            "model": s.model,
            "provider": s.provider,
            "thinkingLevel": s.thinking_level,
            "tokensPerSecond": s.tokens_per_second(),
            "generatedTokens": s.generated_tokens,
            "generationDurationMs": s.generation_duration_ms,
            "sampleCount": s.sample_count
        })).collect::<Vec<_>>(),
        "codexAccounts": data.codex_accounts.iter().map(|a| json!({
            "accountHash": a.account_hash,
            "tokens": {
                "input": a.tokens.input,
                "output": a.tokens.output,
                "cacheRead": a.tokens.cache_read,
                "cacheWrite": a.tokens.cache_write,
                "reasoning": a.tokens.reasoning,
                "total": a.tokens.total()
            },
            "cost": a.cost,
            "paidCost": a.paid_cost,
            "activeMonthCount": a.active_month_count,
            "messageCount": a.message_count,
            "turnCount": a.turn_count,
            "sessionCount": a.session_count,
            "firstDate": a.first_date.map(|date| date.to_string()),
            "latestDate": a.latest_date.map(|date| date.to_string())
        })).collect::<Vec<_>>(),
        "quotaValue": {
            "sampleCount": data.quota_value.sample_count,
            "points": data.quota_value.points.iter().map(|p| json!({
                "date": p.date.to_string(),
                "model": p.model,
                "windowKind": p.window_kind,
                "quotaBurnPct": p.quota_burn_pct,
                "apiValueUsd": p.api_value_usd,
                "subscriptionCostBurned": p.subscription_cost_burned,
                "factor": p.factor,
                "dollarsPerPercent": p.dollars_per_percent,
                "intervalCount": p.interval_count,
                "confidence": p.confidence.as_str()
            })).collect::<Vec<_>>(),
            "modelSummaries": data.quota_value.model_summaries.iter().map(|m| json!({
                "model": m.model,
                "windowKind": m.window_kind,
                "latestDate": m.latest_date.to_string(),
                "quotaBurnPct": m.quota_burn_pct,
                "apiValueUsd": m.api_value_usd,
                "subscriptionCostBurned": m.subscription_cost_burned,
                "factor": m.factor,
                "dollarsPerPercent": m.dollars_per_percent,
                "tokens": {
                    "input": m.tokens.input,
                    "output": m.tokens.output,
                    "cacheRead": m.tokens.cache_read,
                    "cacheWrite": m.tokens.cache_write,
                    "reasoning": m.tokens.reasoning,
                    "total": m.tokens.total()
                },
                "intervalCount": m.interval_count,
                "confidence": m.confidence.as_str()
            })).collect::<Vec<_>>(),
            "intervals": data.quota_value.intervals.iter().map(|q| json!({
                "accountHash": q.account_hash,
                "model": q.model,
                "windowKind": q.window_kind,
                "end": q.end.to_string(),
                "quotaBurnPct": q.quota_burn_pct,
                "apiValueUsd": q.api_value_usd,
                "subscriptionCostBurned": q.subscription_cost_burned,
                "factor": q.factor,
                "dollarsPerPercent": q.dollars_per_percent,
                "tokens": {
                    "input": q.tokens.input,
                    "output": q.tokens.output,
                    "cacheRead": q.tokens.cache_read,
                    "cacheWrite": q.tokens.cache_write,
                    "reasoning": q.tokens.reasoning,
                    "total": q.tokens.total()
                },
                "models": q.models.iter().cloned().collect::<Vec<_>>(),
                "sampleCount": q.sample_count,
                "confidence": q.confidence.as_str()
            })).collect::<Vec<_>>()
        },
        "totals": {
            "tokens": data.total_tokens,
            "cost": data.total_cost
        }
    });

    Ok(serde_json::to_string_pretty(&export_data)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::data::{
        QuotaConfidence, QuotaModelSummary, QuotaValueData, QuotaValueInterval, QuotaValuePoint,
        TokenBreakdown,
    };

    #[test]
    fn test_export_includes_quota_value_without_raw_account_fields() {
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

        let value: serde_json::Value =
            serde_json::from_str(&build_export_json(&data).unwrap()).unwrap();
        let quota = &value["quotaValue"];

        assert_eq!(quota["sampleCount"], 2);
        assert_eq!(quota["intervals"][0]["accountHash"], "acct_hash");
        assert_eq!(quota["intervals"][0]["model"], "gpt-5.4");
        assert_eq!(quota["points"][0]["model"], "gpt-5.4");
        assert_eq!(quota["modelSummaries"][0]["model"], "gpt-5.4");
        assert!(quota["intervals"][0].get("accountId").is_none());
        assert!(quota["intervals"][0].get("email").is_none());
        assert!(quota["intervals"][0].get("auth").is_none());
    }
}
