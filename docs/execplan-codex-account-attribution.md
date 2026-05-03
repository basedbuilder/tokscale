# Codex Account Attribution Tab ExecPlan

This is a living execution plan. Update `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` as work proceeds.

## Purpose

Tokscale should show how much Codex/ChatGPT subscription value came from each local Codex account. The source of truth is not Codex session JSONL metadata. It is the local Codex event ledger at `~/.codex/logs_2.sqlite`, whose `logs.feedback_log_body` rows include `conversation.id`, `user.account_id`, `event.timestamp`, model, and token count fields for recent Codex Desktop/App requests.

After this change, the TUI has an `Accounts` tab that summarizes Codex usage by anonymized account hash, with unmatched sessions shown as `Unattributed`, sessions whose ledger rows mention more than one account shown as `Mixed`, and unreadable/broken ledger state shown as `Ledger error`. Real account rows show both theoretical API cost and estimated paid subscription cost, where paid cost is `$200 * active calendar months with Codex chats` for that account. Non-account buckets still show API cost but leave subscription months/spend blank because assigning subscription spend to them would be misleading. No raw account ids, emails, access tokens, id tokens, or refresh tokens are stored in Tokscale cache or export output.

## Repo And Constraints

- Repo: `/Users/alex/projects/tokscale-local`.
- Root instructions: `AGENTS.md` applies. Prefer `pnpm` for Node work, but this change is Rust-only. Never commit or push unless explicitly requested.
- The worktree is already dirty with local changes in TUI/data files and deleted GitHub workflows. This plan must preserve those local changes and avoid reverting or reformatting unrelated lines.
- Existing relevant files:
  - `crates/tokscale-core/src/sessions/mod.rs` owns `UnifiedMessage`.
  - `crates/tokscale-core/src/lib.rs` owns local parse orchestration and Codex message parsing.
  - `crates/tokscale-core/src/sessions/codex.rs` owns Codex JSONL parsing only; it should not read auth state.
  - `crates/tokscale-cli/src/tui/data/mod.rs` owns TUI aggregation into `UsageData`.
  - `crates/tokscale-cli/src/tui/app.rs` owns tab enum, selection, sorting, and current-row summaries.
  - `crates/tokscale-cli/src/tui/cache.rs` owns TUI cache schema and needs a schema bump when data shape changes.
  - `crates/tokscale-cli/src/tui/export.rs` owns `e` export JSON.
  - `crates/tokscale-cli/src/tui/ui/mod.rs` and `crates/tokscale-cli/src/tui/ui/footer.rs` own TUI rendering dispatch and counts.
- No generated artifacts are involved.

## Current Evidence

- Codex JSONL `session_meta` rows sampled from `~/.codex/sessions` and `~/.codex/archived_sessions` include fields such as `id`, `cwd`, `originator`, `cli_version`, `source`, `model_provider`, and sometimes `agent_nickname`, but not account id or email.
- `~/.codex/auth.json` includes the current account id and JWT claims, but it is not a historical session ledger and must not be parsed by Tokscale for this feature.
- `~/.codex/logs_2.sqlite` has table `logs(id, ts, ts_nanos, level, target, feedback_log_body, module_path, file, line, thread_id, process_uuid, estimated_bytes)`.
- Local proof found recent `codex.sse_event` rows with `conversation.id=...`, token counts, model, `user.account_id="..."`, and `user.email="..."`. Tokscale must extract only `conversation.id` and `user.account_id`, then hash the account id before it leaves the ledger parser.
- On this machine, the ledger contains four distinct account ids and covers many recent April 2026 sessions, but not most older sessions. Therefore unmatched sessions must be visible as `Unattributed`; the implementation must not infer older sessions by date windows.

## Implementation Plan

1. Add account attribution to core messages.
   - Add `codex_account_hash: Option<String>` to `UnifiedMessage` with serde default.
   - Keep constructors defaulting it to `None` so non-Codex parsers and existing tests keep behavior.
   - Add a tiny setter or assign directly inside core parse orchestration.

2. Add a Codex account ledger parser in core.
   - Add `crates/tokscale-core/src/sessions/codex_account.rs` or equivalent Codex-adjacent module.
   - Read `~/.codex/logs_2.sqlite` from the resolved `home_dir` only when Codex is enabled.
   - Query only `feedback_log_body` rows that contain both `conversation.id=` and `user.account_id=`.
   - Parse `conversation.id` and quoted `user.account_id` with small string helpers, not broad regex or JWT parsing.
   - Hash account ids with SHA-256 and expose only a short stable lowercase hex prefix such as 12 chars.
   - If one conversation maps to exactly one account hash, return that hash.
   - If one conversation maps to multiple account hashes, return the literal bucket `mixed`.
   - If the DB file is absent, return an empty attribution map so Codex usage remains visible as `Unattributed`.
   - If the DB file exists but cannot be opened or queried, return a visible attribution error state; core parsing must still return usage totals, but Codex messages should be bucketed as `ledger_error` rather than silently mixed into `Unattributed`.
   - Skip malformed rows only; do not let one malformed row hide good rows.
   - Unit-test line parsing, hashing stability length, conflict-to-`mixed`, missing DB returning empty, existing-but-invalid DB returning a ledger error, and UUID extraction for full stems, prefixed stems, and no-UUID stems.

3. Join attribution into Codex messages.
   - In `parse_all_messages_with_pricing_with_env_strategy` in `crates/tokscale-core/src/lib.rs`, load the Codex ledger once before or around the Codex parse block.
   - Enrich `outcome.messages` after `load_or_parse_codex_source` returns and before extending `all_messages`; do not put account hashes into source-cache entries.
   - For each Codex `UnifiedMessage`, derive the conversation id from `msg.session_id` by extracting the UUID substring. This preserves current `session_id` behavior and avoids changing existing session counting.
   - If a ledger entry exists, set `msg.codex_account_hash`.
   - If the ledger loader returned an error state, set all Codex messages from that parse pass to the literal bucket `ledger_error` so the TUI can show the attribution failure without dropping usage totals.
   - Do not parse or store email, plan type, raw account id, auth tokens, or current `auth.json` claims.

4. Aggregate account usage for the TUI.
   - Add `CodexAccountUsage` to `crates/tokscale-cli/src/tui/data/mod.rs` with `account_hash`, `tokens`, `cost`, `paid_cost`, `active_month_count`, `message_count`, `turn_count`, `session_count`, `first_date`, and `latest_date`.
   - Add `codex_accounts: Vec<CodexAccountUsage>` to `UsageData`.
   - Aggregate only `msg.client == "codex"` messages.
   - Bucket rules: `Some(hash)` -> that hash, `None` -> `unattributed`, `Some("mixed")` -> `mixed`, `Some("ledger_error")` -> `ledger_error`.
   - Count sessions by unique `msg.session_id` per bucket.
   - Count active months by unique local `YYYY-MM` for real account hash buckets and set `paid_cost = active_month_count * 200.0`.
   - Keep `paid_cost` and `active_month_count` empty for `unattributed`, `mixed`, and `ledger_error`.
   - Sort by cost descending, then tokens descending, then account hash ascending.

5. Add the Accounts TUI tab.
   - Add `Tab::Accounts` after `Thinking` and before `Stats` in `crates/tokscale-cli/src/tui/app.rs`.
   - Add `get_sorted_codex_accounts`, current-row summary, item count, and tab navigation updates.
   - Add `crates/tokscale-cli/src/tui/ui/accounts.rs` with a simple table:
     - columns on wide screens: `#`, `Account`, `Tokens`, `API Cost`, `Sub Spend`, `Months`, `Sessions`, `Range`.
     - narrow screens collapse to account, API cost, and subscription spend.
   - Display names: `unattributed` -> `Unattributed`, `mixed` -> `Mixed`, `ledger_error` -> `Ledger error`, other hashes -> `acct <hash>`.
   - Empty state: say no Codex usage is available for the current sources and keep the source/sort/refresh footer path intact.

6. Update cache and export.
   - Bump `CACHE_SCHEMA_VERSION`.
   - Cache `codex_accounts` summaries. Do not cache raw ledger rows or raw account ids.
   - Include `codexAccounts` in export JSON using account hash labels only.

7. Validate.
   - Run focused tests:
     - `cargo test -p tokscale-core codex_account`
     - `cargo test -p tokscale-cli tui`
     - `cargo test -p tokscale-cli cache`
   - Ensure tests cover that attribution is applied after source-cache parsing and never requires raw account ids or emails to be cached/exported.
   - Run a compile check if focused tests do not compile enough of the touched UI:
     - `cargo check -p tokscale-cli`
   - If practical, run `cargo run -q -p tokscale-cli -- clients --json --no-spinner` only if command syntax is confirmed; otherwise skip to avoid broad runtime churn.

## Anti-Slop Boundaries

- Do not add date-window inference for unmatched sessions.
- Do not read `auth.json` for historical attribution.
- Do not store or export raw account ids or emails.
- Do not change Codex session ids to force a join; derive the UUID for joining only.
- Do not add new config or account-label mapping in this pass. Account hashes are enough for the first trustworthy tab; labels can be added after users see which hashes correspond to which accounts.
- Do not refactor existing price/thinking tabs beyond compile-required tab enum changes.
- Do not add a second persisted source of truth for account attribution. The ledger-derived hash lives on enriched `UnifiedMessage` values and summarized TUI/cache/export rows only.

## Progress

- [x] Verified repo instructions and dirty worktree state.
- [x] Verified current source ownership and Codex ledger shape locally.
- [x] Drafted this ExecPlan.
- [x] Independent plan review completed and integrated.
- [x] De-slopify pass completed and integrated.
- [x] Core account ledger parser implemented.
- [x] Core Codex message enrichment implemented.
- [x] TUI aggregation and tab implemented.
- [x] Cache/export updated.
- [x] Focused validation run.

## Surprises & Discoveries

- The account ledger exists in `~/.codex/logs_2.sqlite`, not in session JSONL.
- The ledger timestamps are embedded in `feedback_log_body` as `event.timestamp`; the SQLite `ts` field rendered as epoch-ish in quick queries and should not be used for historical attribution here.
- Local session filenames include a UUID substring; direct full-stem matching misses files with prefixes such as `rollout-...`.

## Decision Log

- Decision: Use hashed account ids only. Rationale: raw account ids and emails are unnecessary for local value summaries and should not enter cache/export.
- Decision: Keep missing ledger as empty attribution. Rationale: usage totals remain primary and accurate; account attribution is optional enrichment and missing coverage must be visible as `Unattributed` rather than guessed.
- Decision: No date-window inference. Rationale: account switches can overlap and local proof found conflicts; date inference would silently misattribute older sessions.
- Decision: Do not add account labels/config in this pass. Rationale: the first implementation should prove trustworthy attribution without adding a second config ownership seam.
- Decision: Enrich after Codex message-cache reads. Rationale: cached parsed sessions should not become stale when the separate account ledger changes, and raw ledger-derived state should not leak into source-cache entries.
- Decision: Use a visible `ledger_error` bucket when an existing ledger cannot be read or queried. Rationale: this preserves primary usage totals without masking a broken attribution source as ordinary unmatched history.
- Decision: Estimate actual subscription cost as `$200` per active calendar month per real account bucket only. Rationale: the user’s subscription spend question is month-bound, and charging only identified accounts with chats avoids showing spend for inactive or unknown accounts.

## Outcomes & Retrospective

- Implemented `Accounts` tab in the TUI with `acct <hash>`, `Unattributed`, `Mixed`, and `Ledger error` buckets.
- Added per-row theoretical API cost and estimated paid subscription cost using `$200` per active month.
- Implemented Codex ledger parsing from `~/.codex/logs_2.sqlite`, hashing only `user.account_id` and joining by UUID extracted from existing Codex session ids.
- Preserved source-cache privacy boundary by applying attribution after Codex source-cache reads and storing only summary buckets in TUI cache/export.
- Validation passed:
  - `cargo test -p tokscale-core codex_account`
  - `cargo test -p tokscale-cli tui`
  - `cargo test -p tokscale-cli cache`
  - `cargo check -p tokscale-cli`
  - `cargo run -q -p tokscale-cli -- --no-spinner clients --json`
- Paid-cost extension validation passed:
  - `cargo test -p tokscale-cli tui::data::tests::test_aggregate_messages_builds_codex_account_usage`
  - `cargo test -p tokscale-cli cache`
  - `cargo check -p tokscale-cli`
- First smoke attempt used the wrong global flag placement, `clients --json --no-spinner`, and failed with `unexpected argument '--no-spinner'`. The corrected global placement succeeded: `--no-spinner clients --json`.

## Revision Notes

- 2026-04-26: Live-code recheck found two Codex parse paths in `crates/tokscale-core/src/lib.rs`: the TUI uses `parse_local_unified_messages_with_pricing`, while some CLI counts use `parse_local_clients`. The first implementation target is the unified-message path because the requested UI tab is TUI data. The plan will not retrofit `ParsedMessage`/`parse_local_clients` unless validation proves the TUI path depends on it.
- 2026-04-26: De-slop pass tightened the cache boundary: account attribution is joined after Codex source-cache loading and is summarized for TUI cache/export only, with no raw ledger rows, emails, or account ids persisted.
- 2026-04-26: Independent review found that query/open failures should not collapse into `Unattributed`, UUID extraction needs direct tests, and the source-cache privacy boundary needs validation. The plan now includes a `ledger_error` bucket and explicit test coverage for those seams.
- 2026-04-26: User clarified the Accounts tab should show theoretical API cost plus actual paid subscription price. Added `paid_cost` and `active_month_count`, using `$200` per active local calendar month with Codex chats in each account bucket.
- 2026-04-26: User screenshot showed high `Unattributed` cost. Investigation found no historical login/logout ledger in `state_5.sqlite` and account-bearing operational rows are request telemetry, not auth history. Updated subscription spend to apply only to real account hash rows; `Unattributed`, `Mixed`, and `Ledger error` keep API cost visible but show blank subscription spend/months.
