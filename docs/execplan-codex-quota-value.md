# Codex Quota Value Tab ExecPlan

This is a living execution plan. Update `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` as work proceeds.

## Purpose

Tokscale should add a TUI tab that proves how much model-normalized API value a Codex subscription produces per observed hidden quota burn. The source of truth is the local Codex session JSONL `event_msg` rows whose `payload.type == "token_count"` and whose payload includes `rate_limits.primary` or `rate_limits.secondary`.

After this change, the TUI has a new `Quota` tab with a rolling 4-day value-factor trail and an interval table. The graph shows the weighted 4-day factor over time, not a naive average of daily factors. The table shows the underlying evidence: observed rate-limit window, quota burn, API-value cost, dollars per 1% quota, model-normalized factor, token totals, sample count, and confidence. The UI must make the metric explicitly inferred: OpenAI does not publish what a quota percent means, so Tokscale reports observed burn-to-value ratios, not an official entitlement.

## Repo And Constraints

- Repo: `/Users/alex/projects/tokscale-local`.
- Root instructions: `AGENTS.md` applies. Prefer `pnpm` for Node work, but this change is Rust-only unless docs or package metadata require updates. Never commit or push unless explicitly requested.
- The worktree is already dirty with local TUI/core changes and deleted GitHub workflows. Preserve unrelated changes and do not revert or reformat adjacent code.
- `.agent/PLANS.md` is referenced by `AGENTS.md` but is absent in this checkout. Use the existing `docs/execplan-codex-account-attribution.md` living-plan shape as the local format reference.
- Existing relevant files:
  - `crates/tokscale-core/src/sessions/codex.rs` owns Codex JSONL parsing and currently parses `token_count` usage but not `rate_limits`.
  - `crates/tokscale-core/src/sessions/codex_account.rs` owns Codex account attribution from `~/.codex/logs_2.sqlite`.
  - `crates/tokscale-core/src/sessions/mod.rs` owns `UnifiedMessage`; avoid stuffing quota-only events into token-message structs.
  - `crates/tokscale-core/src/lib.rs` owns local parse orchestration and Codex source-cache boundaries.
  - `crates/tokscale-core/src/message_cache.rs` owns the source-message cache schema. It has a separate `CACHE_SCHEMA_VERSION` from the TUI cache and must be bumped when `CachedSourceEntry` changes.
  - `crates/tokscale-cli/src/tui/data/mod.rs` owns TUI aggregation into `UsageData`.
  - `crates/tokscale-cli/src/tui/app.rs` owns tab enum, navigation, sorting, and selected-row summaries.
  - `crates/tokscale-cli/src/tui/cache.rs` owns TUI cache schema and needs a schema bump when `UsageData` shape changes.
  - `crates/tokscale-cli/src/tui/export.rs` owns `e` export JSON.
  - `crates/tokscale-cli/src/tui/ui/mod.rs`, `footer.rs`, and per-tab files own TUI rendering dispatch.
  - `crates/tokscale-cli/src/tui/ui/bar_chart.rs` is a token stacked-bar renderer; a quota factor graph should use a small dedicated line/spark renderer rather than bending token chart semantics.
- No generated artifacts are involved.

## Current Evidence

- Local Codex JSONL rows include examples such as `payload.rate_limits.primary.used_percent`, `window_minutes`, `resets_at`, and matching `secondary` values on `token_count` events.
- Some quota rows have `payload.info == null`; the parser must preserve quota observations independently from usage-token observations.
- `primary` has been observed with `window_minutes: 300`; `secondary` has been observed with `window_minutes: 10080`. Do not hardcode those durations as universal constants; parse and display the observed values.
- The existing TUI data model already has daily/hourly/model cost aggregation and account summaries, so quota value should be a derived metric from parsed quota samples plus existing model-normalized cost.
- Current pricing cost is already model-normalized by Tokscale pricing. That should be the numerator for value; do not invent a token-weighting system alongside pricing.
- Account attribution can be reused by applying the same account hash buckets to quota samples as to Codex messages. Do not parse `auth.json` or raw account ids.

## Metric Definition

For each account bucket, rate-limit window kind, and reset window, sort observed samples by timestamp. Build intervals only when the next sample has the same `window_kind`, same `resets_at`, and a strictly higher `used_percent`.

For interval `i` from sample `a` to sample `b`:

```text
quota_burn_pct_i = b.used_percent - a.used_percent
api_value_usd_i = sum Codex message cost where a.timestamp < message.timestamp <= b.timestamp
quota_dollars_per_percent_i = api_value_usd_i / quota_burn_pct_i
```

For a subscription-normalized factor, amortize the subscription price over the observed window duration:

```text
window_subscription_budget_i =
  monthly_subscription_usd * window_minutes / average_month_minutes

subscription_cost_burned_i =
  window_subscription_budget_i * quota_burn_pct_i / 100

factor_i =
  api_value_usd_i / subscription_cost_burned_i
```

Use `average_month_minutes = 365.2425 / 12 * 24 * 60`. Reuse or move the existing `CODEX_MONTHLY_SUBSCRIPTION_COST_USD` from `crates/tokscale-cli/src/tui/data/mod.rs`; do not add a second `$200` source of truth.

The rolling 4-day trail must be a weighted ratio over all intervals ending in the last 4 calendar days:

```text
factor_4d(day) =
  sum(api_value_usd_i over window)
  /
  sum(subscription_cost_burned_i over window)
```

Do not average `factor_i` values directly. That overweights tiny 1% or low-cost intervals and creates fake spikes.

## User-Facing Design

Add a new `Quota` tab after `Accounts` and before `Stats`.

Wide layout:

- Top summary strip:
  - `4d Factor`: latest weighted 4-day factor, such as `3.8x`.
  - `API Value`: rolling 4-day model-normalized API value.
  - `Quota Burn`: rolling 4-day observed burn, split by selected window kind.
  - `$ / 1%`: rolling value per 1% quota burn.
  - `Samples`: sample count and confidence label.
- Main graph:
  - Title: `4d Value Factor`.
  - X axis: last observed days, oldest to newest.
  - Y axis: factor multiple.
  - Main line: rolling 4-day weighted factor.
  - Faint markers or small vertical ticks: raw interval factors, clipped at the chart max and marked as clipped if needed.
  - Separate color/style for `primary` and `secondary` if both are displayed. If the UI needs one default, default to `secondary` because it is the longer window and less noisy.
- Table:
  - Columns: `End`, `Window`, `Burn`, `API Value`, `Factor`, `$ / 1%`, `Tokens`, `Models`, `Samples`, `Confidence`.
  - Default sort: newest interval day descending.
  - `c` sorts by factor/cost-style value, `t` sorts by tokens, `d` sorts by date, matching existing keyboard conventions.

Narrow layout:

- Keep the same top line but collapse summary to `4d Factor`, `Burn`, and `API Value`.
- Graph remains visible above the table.
- Table columns collapse to `End`, `Window`, `Burn`, `Factor`, `Value`.

Empty state:

- If Codex has no parsed `rate_limits`, show: `No Codex quota observations found. Refresh after a Codex response with rate-limit telemetry.`
- If quota samples exist but no positive burn intervals exist, show: `Quota samples found, but no positive quota burn yet.`
- Do not fall back to token-only estimates.

Confidence labels:

- `High`: at least 3 positive-burn intervals and at least 2% total burn in the rolling window.
- `Medium`: at least 2 positive-burn intervals and at least 1% total burn.
- `Low`: anything smaller. Display it, but visually mute the row and graph point.

## Implementation Plan

1. Add a quota sample type in core.
   - Add a Codex-specific `RateLimitSample` or `CodexQuotaSample` type outside `UnifiedMessage`.
   - Fields: `client`, `session_id`, `codex_account_hash`, `timestamp`, `date`, `provider_id`, `model_id`, `window_kind`, `used_percent`, `window_minutes`, and `resets_at`.
   - Omit credits and plan metadata in this pass. They are not needed for the factor graph and broadening the schema would weaken the privacy boundary without proven UI value.
   - Keep raw account ids, emails, access tokens, id tokens, and refresh tokens out of this type.

2. Parse Codex `rate_limits` from JSONL.
   - Extend `CodexPayload` in `crates/tokscale-core/src/sessions/codex.rs` to deserialize `rate_limits`.
   - Emit quota samples for `primary` and `secondary` when present, even when `info` is `null`.
   - Attach the current model/provider/session context where available; use `unknown` only for display grouping, not as a fallback cost source.
   - Unit-test parsing with `info == null`, with both `primary` and `secondary`, and with malformed/missing rate-limit fields skipped visibly in parser tests.

3. Preserve source-cache and account boundaries.
   - Choose one data path: introduce a new core return type such as `ParsedLocalUsage { messages, quota_samples }`.
   - Add a new public/internal API such as `parse_local_usage_with_pricing(options, pricing)` that returns `ParsedLocalUsage`.
   - Keep `parse_local_unified_messages_with_pricing` and `parse_local_unified_messages` as compatibility wrappers that call the new API and return only `messages`.
   - Extend internal Codex parse outcomes and `CachedSourceEntry` to carry unauthenticated quota samples alongside messages. These samples may include session id, timestamp, model/provider, and rate-limit window values, but no account hash until after ledger attribution.
   - Bump the core source-cache `CACHE_SCHEMA_VERSION` in `crates/tokscale-core/src/message_cache.rs` and add a source-cache round-trip test that proves quota samples persist without account hashes.
   - Apply `codex_account_hash` to quota samples after the account ledger join, matching message attribution.
   - This preserves one file-parse pass and keeps account attribution non-stale because account hashes are joined after reading source-cache entries.

4. Aggregate quota intervals in TUI data.
   - Add `QuotaValueData`, `QuotaValueInterval`, `QuotaValuePoint`, and `QuotaConfidence` structs in `crates/tokscale-cli/src/tui/data/mod.rs`.
   - Add `quota_value` or `quota_values` to `UsageData`.
   - Match Codex message costs into sample intervals by `account_hash`, `window_kind`, timestamp range, and reset window.
   - Reset segmentation when `used_percent` decreases, `resets_at` changes, or `window_minutes` changes.
   - Skip zero-burn intervals from factor calculations; keep sample counts so the empty/low-confidence state is explainable.
   - Compute daily rows and 4-day weighted points from intervals, not from daily totals.

5. Add the `Quota` TUI tab.
   - Add `Tab::Quota` after `Accounts` in `crates/tokscale-cli/src/tui/app.rs` and update `all`, `next`, `prev`, `as_str`, `short_name`, item counts, selected-row summary, and tests.
   - Add `crates/tokscale-cli/src/tui/ui/quota.rs`.
   - Add a small dedicated renderer for the factor line/spark graph in `quota.rs` or `ui/factor_chart.rs`; do not add a new chart dependency.
   - Keep graph and table in one tab so the pretty trend and the proof rows stay together.
   - Keep copy short and precise: `4d Value Factor`, `API Value`, `Quota Burn`, `Inferred`.

6. Update cache and export.
   - Bump `CACHE_SCHEMA_VERSION`.
   - Cache the same final `QuotaValueData` shape used by `UsageData`; do not cache both independently recomputable summaries and source rows that could drift. If the UI needs intervals and graph points, store those final rows once and derive header text from them.
   - Include `quotaValue` in export JSON with account hashes only and no raw account/auth data.
   - Add cache round-trip tests so stale cache shape failures are caught.
   - Add an export unit test around `build_export_json` that proves `quotaValue` is present and contains no raw account/auth fields.

7. Add focused tests.
   - Core parser tests for quota sample extraction from Codex JSONL.
   - Core source-cache tests for quota sample round-trip without account hashes.
   - Core compatibility tests that `parse_local_unified_messages_with_pricing` still returns the same message vector shape.
   - TUI data tests for interval construction, reset segmentation, zero-burn handling, and 4-day weighted factor math.
   - TUI data tests that disabled Codex sources produce empty quota data even if other clients are selected.
   - App tests for `Tab::Quota` navigation and default sorting.
   - Footer tests or focused app/UI assertions that `Quota` does not fall through to the default `models` count label.
   - Cache/export tests for `quotaValue`.

8. Validate.
   - Run:
     - `cargo test -p tokscale-core codex`
     - `cargo test -p tokscale-cli tui::data`
     - `cargo test -p tokscale-cli tui`
     - `cargo test -p tokscale-cli cache`
     - `cargo test -p tokscale-cli export`
     - `cargo check -p tokscale-cli`
   - Run a local smoke after tests:
     - `cargo run -q -p tokscale-cli -- --no-spinner --help`
   - Treat the smoke as a non-interactive CLI sanity check only. Prove tab presence through app/UI tests, because non-interactive runs can route to a report path instead of the TUI.

## Anti-Slop Boundaries

- Do not infer quota burn from token usage when `rate_limits` are absent.
- Do not treat one `used_percent` as official token capacity.
- Do not average factors directly; always use weighted ratios.
- Do not add a web scraper, browser automation, or API polling path for quota in this pass.
- Do not parse `auth.json` or expose raw account identifiers.
- Do not add a generic provider abstraction until another provider has an observed quota telemetry source in local logs.
- Do not refactor existing `Prices`, `Thinking`, or `Accounts` tabs beyond enum/cache/export changes required by the new tab.
- Do not add configurable plan prices in this pass unless implementation proves the existing `$200` constant is already wrong for the current Codex product; if pricing configurability is needed, call it out as a larger follow-up.

## Progress

- [x] Verified repo instructions and current dirty worktree state.
- [x] Verified `.agent/PLANS.md` is absent and existing `docs/execplan-codex-account-attribution.md` is the practical local ExecPlan format.
- [x] Verified current TUI tab/cache/export ownership.
- [x] Verified local Codex JSONL has `payload.rate_limits.primary/secondary` observations.
- [x] Drafted this ExecPlan.
- [x] Independent anti-slop review completed and integrated.
- [x] Implementation started.
- [x] Core quota sample parser/data path compiles.
- [x] Focused validation run.
- [x] Core parser extracts quota samples independently from token usage rows.
- [x] Core source cache persists quota samples without account hashes.
- [x] TUI aggregation computes positive-burn intervals and weighted 4-day points.
- [x] `Quota` tab, footer count, TUI cache, and export JSON are wired.

## Surprises & Discoveries

- Codex quota telemetry is in session JSONL `token_count` payloads as `rate_limits`, not in the account ledger.
- Some useful quota samples have `info == null`, so quota parsing must not depend on token usage parsing succeeding.
- The repo does not contain `.agent/PLANS.md`; the existing account-attribution ExecPlan is the local format reference.
- Existing TUI has a newly added `Accounts` tab, so `Quota` should sit after `Accounts` to keep subscription/account views adjacent.
- Independent review found the first draft left the core data path ambiguous. The plan now chooses a new `ParsedLocalUsage` return path with compatibility wrappers and source-cache quota sample storage.
- Adversarial preflight found the core source cache has its own schema version and the first draft only named the TUI cache. The plan now requires both schema bumps and tests.
- Adversarial preflight found the non-interactive smoke command cannot prove TUI tab presence. Tab presence must be proven by app/UI tests instead.
- Adversarial preflight noted quota samples with missing timestamps need explicit handling. Implementation uses only explicitly timestamped quota observations, so cached quota sample dates do not depend on file-mtime fallback refresh.
- Implementation found `parse_local_clients` remains a message-only parse/export path. Quota samples are exposed through the richer `parse_local_usage_with_pricing` path used by the TUI data loader; the existing message-only APIs remain compatibility wrappers.

## Decision Log

- Decision: Use observed `rate_limits` only. Rationale: token-only inference would mask the exact uncertainty the feature is meant to expose.
- Decision: Keep quota samples separate from `UnifiedMessage`. Rationale: quota observations can exist without usage info, and `UnifiedMessage` represents billable model work.
- Decision: Default graph focus to the `secondary` window when both windows are available. Rationale: the longer reset window is less noisy and better matches subscription value, while `primary` can still be displayed as a table/window row.
- Decision: Use weighted 4-day ratios. Rationale: direct factor averages overemphasize tiny quota deltas and produce misleading spikes.
- Decision: Use time-amortized subscription budget per quota window. Rationale: a 5-hour primary window and 7-day secondary window cannot both be treated as 100% of a monthly subscription.
- Decision: Keep the first pass Codex-specific. Rationale: no other provider quota telemetry source has been verified in this repo, and a generic abstraction would add moving parts without evidence.
- Decision: Store unauthenticated quota samples in source cache and join account hashes after cache reads. Rationale: this keeps one parse pass without persisting raw account state or stale account attribution.
- Decision: Reuse the existing Codex subscription price constant instead of adding a new one. Rationale: the feature should have one subscription-price source of truth.
- Decision: Cache final quota value rows once in `UsageData` instead of caching multiple recomputable layers. Rationale: this avoids summary/interval drift in the TUI cache.
- Decision: Skip quota samples without explicit event timestamps. Rationale: quota value intervals are evidence rows; file mtime fallback would create stale or misleading interval dates.

## Outcomes & Retrospective

- Implemented `CodexQuotaSample` parsing from Codex `rate_limits.primary` and `rate_limits.secondary`, including rows where `payload.info == null`.
- Added `ParsedLocalUsage` and preserved existing message-only parse APIs as wrappers.
- Extended the core source cache and TUI cache schema versions with quota sample/value round-trip coverage.
- Added account-hash attribution for quota samples after the Codex account ledger join, matching message attribution and avoiding raw account ids.
- Added `Quota` TUI data, tab dispatch, footer count, sparkline/table rendering, selected-row summary, cache/export JSON, and focused tests for positive intervals, zero/negative burn skipping, and weighted 4-day factor math.
- Validation passed:
  - `cargo test -p tokscale-core codex`
  - `cargo test -p tokscale-core source_message_cache_round_trip`
  - `cargo test -p tokscale-cli tui::data`
  - `cargo test -p tokscale-cli tui`
  - `cargo test -p tokscale-cli cache`
  - `cargo test -p tokscale-cli export`
  - `cargo check -p tokscale-cli`
  - `cargo run -q -p tokscale-cli -- --no-spinner --help`

## Revision Notes

- 2026-04-26: Initial draft based on live inspection of TUI data/cache/app structure and local Codex JSONL `rate_limits` telemetry.
- 2026-04-26: Independent anti-slop review tightened the plan around one core data path, one subscription-price constant, no credit/plan metadata, and deterministic Codex-disabled validation.
- 2026-04-26: Implement preflight adversarial review tightened the plan around core source-cache schema versioning, single cached quota value shape, footer label coverage, export tests, and non-interactive smoke limits.
- 2026-04-26: Implementation completed with the smoke command narrowed to `--help` so validation stays non-interactive while tab behavior is covered by TUI/app tests.
