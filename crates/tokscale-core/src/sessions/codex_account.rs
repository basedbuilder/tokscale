use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

use super::{CodexQuotaSample, UnifiedMessage};

pub const MIXED_ACCOUNT_BUCKET: &str = "mixed";
pub const LEDGER_ERROR_BUCKET: &str = "ledger_error";

#[derive(Debug, Clone, Default)]
pub struct CodexAccountLedger {
    pub conversations: HashMap<String, String>,
    pub failed: bool,
}

pub fn load_codex_account_ledger(home_dir: &str) -> CodexAccountLedger {
    let path = PathBuf::from(home_dir).join(".codex").join("logs_2.sqlite");
    load_codex_account_ledger_from_path_with_range(&path, None, None)
}

pub fn load_codex_account_ledger_with_range(
    home_dir: &str,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
) -> CodexAccountLedger {
    let path = PathBuf::from(home_dir).join(".codex").join("logs_2.sqlite");
    load_codex_account_ledger_from_path_with_range(&path, since_ts, until_ts)
}

pub fn load_codex_account_ledger_for_conversations(
    home_dir: &str,
    conversation_ids: &BTreeSet<String>,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
) -> CodexAccountLedger {
    let path = PathBuf::from(home_dir).join(".codex").join("logs_2.sqlite");
    if !path.exists() || conversation_ids.is_empty() {
        return CodexAccountLedger::default();
    }

    match read_codex_account_ledger_for_conversations(&path, conversation_ids, since_ts, until_ts) {
        Ok(conversations) => CodexAccountLedger {
            conversations,
            failed: false,
        },
        Err(_) => CodexAccountLedger {
            conversations: HashMap::new(),
            failed: true,
        },
    }
}

#[cfg(test)]
pub(crate) fn load_codex_account_ledger_from_path(path: &Path) -> CodexAccountLedger {
    load_codex_account_ledger_from_path_with_range(path, None, None)
}

fn load_codex_account_ledger_from_path_with_range(
    path: &Path,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
) -> CodexAccountLedger {
    if !path.exists() {
        return CodexAccountLedger::default();
    }

    match read_codex_account_ledger(path, since_ts, until_ts) {
        Ok(conversations) => CodexAccountLedger {
            conversations,
            failed: false,
        },
        Err(_) => CodexAccountLedger {
            conversations: HashMap::new(),
            failed: true,
        },
    }
}

fn read_codex_account_ledger(
    path: &Path,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
) -> rusqlite::Result<HashMap<String, String>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut sql = String::from(
        "SELECT thread_id, feedback_log_body FROM logs \
         WHERE target = 'codex_otel.log_only' \
         AND thread_id IS NOT NULL \
         AND feedback_log_body LIKE '%user.account_id=%'",
    );
    if since_ts.is_some() {
        sql.push_str(" AND ts >= ?");
    }
    if until_ts.is_some() {
        sql.push_str(" AND ts <= ?");
    }
    let params = since_ts.into_iter().chain(until_ts).collect::<Vec<_>>();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
    let mut by_conversation: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    while let Some(row) = rows.next()? {
        let Some(thread_id) = row.get::<_, Option<String>>(0)? else {
            continue;
        };
        let conversation_id = if is_uuid(&thread_id) {
            thread_id
        } else {
            let Some(body) = row.get::<_, Option<String>>(1)? else {
                continue;
            };
            let Some((conversation_id, account_id)) = parse_codex_account_event(&body) else {
                continue;
            };
            by_conversation
                .entry(conversation_id)
                .or_default()
                .insert(short_account_hash(&account_id));
            continue;
        };
        let Some(body) = row.get::<_, Option<String>>(1)? else {
            continue;
        };
        let Some(account_id) = extract_quoted_after(&body, "user.account_id=\"") else {
            continue;
        };
        by_conversation
            .entry(conversation_id)
            .or_default()
            .insert(short_account_hash(account_id));
    }

    Ok(by_conversation
        .into_iter()
        .map(|(conversation_id, hashes)| {
            let bucket = if hashes.len() == 1 {
                hashes.into_iter().next().unwrap_or_default()
            } else {
                MIXED_ACCOUNT_BUCKET.to_string()
            };
            (conversation_id, bucket)
        })
        .collect())
}

fn read_codex_account_ledger_for_conversations(
    path: &Path,
    conversation_ids: &BTreeSet<String>,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
) -> rusqlite::Result<HashMap<String, String>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut sql = String::from(
        "SELECT feedback_log_body FROM logs INDEXED BY idx_logs_thread_id_ts \
         WHERE thread_id = ? \
         AND target = 'codex_otel.log_only' \
         AND feedback_log_body LIKE '%user.account_id=%'",
    );
    if since_ts.is_some() {
        sql.push_str(" AND ts >= ?");
    }
    if until_ts.is_some() {
        sql.push_str(" AND ts <= ?");
    }
    sql.push_str(" ORDER BY ts DESC, ts_nanos DESC, id DESC");

    let mut stmt = conn.prepare(&sql)?;
    let mut by_conversation: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for conversation_id in conversation_ids {
        let mut params = vec![SqlValue::Text(conversation_id.clone())];
        if let Some(since_ts) = since_ts {
            params.push(SqlValue::Integer(since_ts));
        }
        if let Some(until_ts) = until_ts {
            params.push(SqlValue::Integer(until_ts));
        }

        let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            let Some(body) = row.get::<_, Option<String>>(0)? else {
                continue;
            };
            let Some(account_id) = extract_quoted_after(&body, "user.account_id=\"") else {
                continue;
            };
            by_conversation
                .entry(conversation_id.clone())
                .or_default()
                .insert(short_account_hash(account_id));
        }
    }

    Ok(by_conversation
        .into_iter()
        .map(|(conversation_id, hashes)| {
            let bucket = if hashes.len() == 1 {
                hashes.into_iter().next().unwrap_or_default()
            } else {
                MIXED_ACCOUNT_BUCKET.to_string()
            };
            (conversation_id, bucket)
        })
        .collect())
}

pub(crate) fn parse_codex_account_event(body: &str) -> Option<(String, String)> {
    let conversation_id = extract_conversation_id(body)?;
    let account_id = extract_quoted_after(body, "user.account_id=\"")?;
    Some((conversation_id.to_string(), account_id.to_string()))
}

pub(crate) fn conversation_id_from_session_id(session_id: &str) -> Option<&str> {
    let bytes = session_id.as_bytes();
    if bytes.len() < 36 {
        return None;
    }

    for start in 0..=bytes.len() - 36 {
        let candidate = &session_id[start..start + 36];
        if is_uuid(candidate) {
            return Some(candidate);
        }
    }
    None
}

pub fn apply_codex_account_ledger(messages: &mut [UnifiedMessage], ledger: &CodexAccountLedger) {
    for message in messages {
        if message.client != "codex" {
            continue;
        }
        if ledger.failed {
            message.codex_account_hash = Some(LEDGER_ERROR_BUCKET.to_string());
            continue;
        }
        let Some(conversation_id) = conversation_id_from_session_id(&message.session_id) else {
            continue;
        };
        if let Some(account_hash) = ledger.conversations.get(conversation_id) {
            message.codex_account_hash = Some(account_hash.clone());
        }
    }
}

pub fn apply_codex_account_ledger_to_quota_samples(
    samples: &mut [CodexQuotaSample],
    ledger: &CodexAccountLedger,
) {
    for sample in samples {
        if ledger.failed {
            sample.codex_account_hash = Some(LEDGER_ERROR_BUCKET.to_string());
            continue;
        }
        let Some(conversation_id) = conversation_id_from_session_id(&sample.session_id) else {
            continue;
        };
        if let Some(account_hash) = ledger.conversations.get(conversation_id) {
            sample.codex_account_hash = Some(account_hash.clone());
        }
    }
}

fn extract_conversation_id(body: &str) -> Option<&str> {
    let rest = body.split_once("conversation.id=")?.1;
    let end = rest
        .find(|ch: char| ch.is_whitespace() || ch == '"' || ch == ',' || ch == '}')
        .unwrap_or(rest.len());
    let candidate = &rest[..end];
    is_uuid(candidate).then_some(candidate)
}

fn extract_quoted_after<'a>(body: &'a str, needle: &str) -> Option<&'a str> {
    let rest = body.split_once(needle)?.1;
    let end = rest.find('"')?;
    let value = &rest[..end];
    (!value.is_empty()).then_some(value)
}

fn short_account_hash(account_id: &str) -> String {
    let hash = Sha256::digest(account_id.as_bytes());
    let mut out = String::with_capacity(12);
    for byte in hash.iter().take(6) {
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
    out
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + (nibble - 10)) as char,
    }
}

fn is_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.chars().enumerate().all(|(index, ch)| match index {
        8 | 13 | 18 | 23 => ch == '-',
        _ => ch.is_ascii_hexdigit(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenBreakdown;
    use tempfile::TempDir;

    const CONVERSATION_ID: &str = "019dc808-1111-7222-8333-944455556666";

    #[test]
    fn parses_conversation_and_account_id() {
        let body = format!(
            r#"event.timestamp="2026-04-26T00:00:00Z" conversation.id={CONVERSATION_ID} user.account_id="acct_a" user.email="secret@example.com""#
        );

        let parsed = parse_codex_account_event(&body).unwrap();

        assert_eq!(parsed.0, CONVERSATION_ID);
        assert_eq!(parsed.1, "acct_a");
    }

    #[test]
    fn extracts_uuid_from_full_and_prefixed_session_ids() {
        assert_eq!(
            conversation_id_from_session_id(CONVERSATION_ID),
            Some(CONVERSATION_ID)
        );
        assert_eq!(
            conversation_id_from_session_id(&format!("rollout-2026-04-26-{CONVERSATION_ID}")),
            Some(CONVERSATION_ID)
        );
        assert_eq!(
            conversation_id_from_session_id("session-without-uuid"),
            None
        );
    }

    #[test]
    fn hashes_accounts_without_exposing_raw_id() {
        let hash = short_account_hash("acct_secret");

        assert_eq!(hash.len(), 12);
        assert_eq!(hash, short_account_hash("acct_secret"));
        assert_ne!(hash, "acct_secret");
        assert!(hash.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn maps_conflicting_accounts_to_mixed_bucket() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("logs_2.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE logs (
                ts INTEGER NOT NULL DEFAULT 0,
                target TEXT NOT NULL DEFAULT 'codex_otel.log_only',
                thread_id TEXT,
                feedback_log_body TEXT
            )",
            [],
        )
        .unwrap();
        for account in ["acct_a", "acct_b"] {
            conn.execute(
                "INSERT INTO logs (thread_id, feedback_log_body) VALUES (?1, ?2)",
                rusqlite::params![
                    CONVERSATION_ID,
                    format!(r#"target=codex.sse_event user.account_id="{account}""#)
                ],
            )
            .unwrap();
        }
        drop(conn);

        let ledger = load_codex_account_ledger_from_path(&db_path);

        assert!(!ledger.failed);
        assert_eq!(
            ledger
                .conversations
                .get(CONVERSATION_ID)
                .map(String::as_str),
            Some(MIXED_ACCOUNT_BUCKET)
        );
    }

    #[test]
    fn missing_db_is_empty_but_invalid_db_is_error() {
        let dir = TempDir::new().unwrap();
        let missing = load_codex_account_ledger_from_path(&dir.path().join("missing.sqlite"));
        assert!(!missing.failed);
        assert!(missing.conversations.is_empty());

        let invalid_path = dir.path().join("logs_2.sqlite");
        std::fs::write(&invalid_path, b"not sqlite").unwrap();
        let invalid = load_codex_account_ledger_from_path(&invalid_path);
        assert!(invalid.failed);
        assert!(invalid.conversations.is_empty());
    }

    #[test]
    fn applies_hash_after_parsing_messages() {
        let mut ledger = CodexAccountLedger::default();
        ledger
            .conversations
            .insert(CONVERSATION_ID.to_string(), "abcdef123456".to_string());
        let mut messages = vec![UnifiedMessage::new(
            "codex",
            "gpt-5",
            "openai",
            format!("rollout-{CONVERSATION_ID}"),
            1_700_000_000_000,
            TokenBreakdown::default(),
            0.0,
        )];

        apply_codex_account_ledger(&mut messages, &ledger);

        assert_eq!(
            messages[0].codex_account_hash.as_deref(),
            Some("abcdef123456")
        );
    }

    #[test]
    fn failed_ledger_marks_codex_messages_as_ledger_error() {
        let ledger = CodexAccountLedger {
            conversations: HashMap::new(),
            failed: true,
        };
        let mut messages = vec![UnifiedMessage::new(
            "codex",
            "gpt-5",
            "openai",
            CONVERSATION_ID,
            1_700_000_000_000,
            TokenBreakdown::default(),
            0.0,
        )];

        apply_codex_account_ledger(&mut messages, &ledger);

        assert_eq!(
            messages[0].codex_account_hash.as_deref(),
            Some(LEDGER_ERROR_BUCKET)
        );
    }
}
