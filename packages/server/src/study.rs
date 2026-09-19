//! Durable personal study sync API (`/api/study/*`).
//!
//! Shares the SQLite database and connection with the online-exam relay, but
//! keeps all data in permanent `study_*` tables that the 7-day relay cleanup
//! never touches. Every endpoint requires `Authorization: Bearer
//! <STUDY_SYNC_TOKEN>` and fails closed with 503 when that env var is unset or
//! blank. The token is never logged or returned in any response.

use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::{Arc, MutexGuard};

use crate::routes::AppState;

const MAX_BATCH_ATTEMPTS: usize = 500;
const MAX_BODY_BYTES: usize = 12 * 1024 * 1024;
const DEFAULT_PAGE_LIMIT: i64 = 100;
const MAX_PAGE_LIMIT: i64 = 500;
const MAX_KEY_LEN: usize = 512;
const MAX_CONSUMER_LEN: usize = 128;
const MAX_TAG_LEN: usize = 256;
const MAX_ANSWER_LEN: usize = 32 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024;
const MAX_BANK_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;
const MAX_BANK_QUESTIONS: usize = 5000;
const MAX_BANK_NAME_LEN: usize = 500;
const MAX_BANKS_LISTED: i64 = 1000;

type Err = (StatusCode, String);

fn err(status: StatusCode, code: &str) -> Err {
    (status, serde_json::json!({ "error": code }).to_string())
}

fn err_msg(status: StatusCode, code: &str, message: impl std::fmt::Display) -> Err {
    (
        status,
        serde_json::json!({ "error": code, "message": message.to_string() }).to_string(),
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn sha256_hex(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn init_tables(conn: &Connection) -> Result<(), String> {
    let had_review_events = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'study_review_events'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .is_some();

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS study_attempts (
          seq INTEGER PRIMARY KEY AUTOINCREMENT,
          idempotency_key TEXT NOT NULL UNIQUE,
          session_key TEXT NOT NULL,
          device_id TEXT NOT NULL DEFAULT '',
          bank_key TEXT NOT NULL DEFAULT '',
          question_key TEXT NOT NULL,
          original_question_id TEXT NOT NULL DEFAULT '',
          session_question_id TEXT NOT NULL DEFAULT '',
          question_snapshot TEXT NOT NULL DEFAULT '',
          user_answer TEXT NOT NULL DEFAULT '',
          user_answer_is_null INTEGER NOT NULL DEFAULT 0,
          correct_answer TEXT NOT NULL DEFAULT '',
          is_correct INTEGER,
          flagged INTEGER NOT NULL DEFAULT 0,
          subject TEXT,
          chapter TEXT,
          knowledge_point TEXT,
          duration_ms INTEGER NOT NULL DEFAULT 0,
          submitted_at INTEGER NOT NULL,
          created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_study_attempts_session ON study_attempts(session_key);
        CREATE INDEX IF NOT EXISTS idx_study_attempts_question ON study_attempts(question_key);
        CREATE TABLE IF NOT EXISTS study_review_events (
          seq INTEGER PRIMARY KEY AUTOINCREMENT,
          attempt_seq INTEGER NOT NULL,
          reason TEXT NOT NULL,
          created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_study_review_events_attempt ON study_review_events(attempt_seq);
        CREATE TABLE IF NOT EXISTS study_consumers (
          consumer TEXT PRIMARY KEY,
          cursor INTEGER NOT NULL DEFAULT 0,
          updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS study_sessions (
          session_key TEXT PRIMARY KEY,
          started_at INTEGER,
          finished_at INTEGER,
          updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS study_banks (
          bank_key TEXT PRIMARY KEY,
          name TEXT NOT NULL,
          payload TEXT NOT NULL,
          content_hash TEXT NOT NULL DEFAULT '',
          question_count INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| e.to_string())?;

    // Forward-compatible migration for databases created by early study-sync builds.
    let has_device_id = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .map_err(|e| e.to_string())?;
        let cols = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?;
        let found = cols.filter_map(Result::ok).any(|name| name == "device_id");
        found
    };
    if !has_device_id {
        conn.execute(
            "ALTER TABLE study_attempts ADD COLUMN device_id TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| e.to_string())?;
    }

    let has_bank_key = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .map_err(|e| e.to_string())?;
        let cols = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?;
        let found = cols.filter_map(Result::ok).any(|name| name == "bank_key");
        found
    };
    if !has_bank_key {
        conn.execute(
            "ALTER TABLE study_attempts ADD COLUMN bank_key TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| e.to_string())?;
    }

    let has_duration_ms = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .map_err(|e| e.to_string())?;
        let cols = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?;
        let found = cols
            .filter_map(Result::ok)
            .any(|name| name == "duration_ms");
        found
    };
    if !has_duration_ms {
        conn.execute(
            "ALTER TABLE study_attempts ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| e.to_string())?;
    }

    let has_original_question_id = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .map_err(|e| e.to_string())?;
        let cols = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?;
        let found = cols
            .filter_map(Result::ok)
            .any(|name| name == "original_question_id");
        found
    };
    if !has_original_question_id {
        conn.execute(
            "ALTER TABLE study_attempts ADD COLUMN original_question_id TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| e.to_string())?;
    }

    let has_user_answer_is_null = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .map_err(|e| e.to_string())?;
        let cols = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?;
        let found = cols
            .filter_map(Result::ok)
            .any(|name| name == "user_answer_is_null");
        found
    };
    if !has_user_answer_is_null {
        conn.execute(
            "ALTER TABLE study_attempts ADD COLUMN user_answer_is_null INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| e.to_string())?;
    }

    let has_bank_content_hash = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(study_banks)")
            .map_err(|e| e.to_string())?;
        let cols = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?;
        let found = cols
            .filter_map(Result::ok)
            .any(|name| name == "content_hash");
        found
    };
    if !has_bank_content_hash {
        conn.execute(
            "ALTER TABLE study_banks ADD COLUMN content_hash TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| e.to_string())?;
    }

    if !had_review_events {
        // Backfill review-worthy attempts from early builds and reset old attempt-seq cursors.
        conn.execute(
            "INSERT INTO study_review_events (attempt_seq, reason, created_at)
             SELECT seq, CASE WHEN is_correct = 0 THEN 'wrong' ELSE 'flagged' END, created_at
             FROM study_attempts
             WHERE is_correct = 0 OR flagged = 1",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM study_consumers", [])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn lock_conn(state: &AppState) -> Result<MutexGuard<'_, Connection>, Err> {
    state
        .relay
        .conn
        .lock()
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "db_lock"))
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

fn bearer_matches(provided: &str, expected: &str) -> bool {
    // Compare digests so the comparison never short-circuits on token bytes.
    match provided.strip_prefix("Bearer ") {
        Some(rest) => sha256_hex(rest) == sha256_hex(expected),
        None => false,
    }
}

fn err_response(status: StatusCode, code: &str) -> Response {
    let body = serde_json::json!({ "error": code }).to_string();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn study_auth(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let token = state.study_token.trim();
    if token.is_empty() {
        return err_response(StatusCode::SERVICE_UNAVAILABLE, "study_sync_disabled");
    }
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| bearer_matches(v, token));
    if !authorized {
        return err_response(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    next.run(req).await
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/attempts/batch", post(attempts_batch_handler))
        .route("/attempts/{id}/flag", post(attempt_flag_handler))
        .route("/sessions/finish", post(sessions_finish_handler))
        .route("/sessions/{session_key}/finish", post(sessions_finish_path_handler))
        .route("/sessions/latest", get(sessions_latest_handler))
        .route("/feed", get(feed_handler))
        .route("/feed/ack", post(feed_ack_handler))
        .route(
            "/questions/{question_key}/history",
            get(question_history_handler),
        )
        .route("/banks/import", post(banks_import_handler))
        .route("/banks", get(banks_list_handler).post(banks_import_handler))
        .route("/banks/{bank_key}", get(banks_get_handler))
        .route("/health", get(health_handler))
        .route("/status", get(health_handler))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(from_fn_with_state(state, study_auth))
}

// ---------------------------------------------------------------------------
// Attempts: batch upload with idempotency
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AttemptInput {
    #[serde(rename = "idempotencyKey")]
    pub idempotency_key: Option<String>,
    #[serde(rename = "sessionKey")]
    pub session_key: Option<String>,
    #[serde(rename = "deviceId")]
    pub device_id: Option<String>,
    #[serde(rename = "bankKey")]
    pub bank_key: Option<String>,
    #[serde(rename = "questionKey")]
    pub question_key: Option<String>,
    #[serde(rename = "originalQuestionId")]
    pub original_question_id: Option<String>,
    #[serde(rename = "sessionQuestionId")]
    pub session_question_id: Option<String>,
    #[serde(rename = "questionSnapshot")]
    pub question_snapshot: Option<serde_json::Value>,
    #[serde(rename = "userAnswer")]
    pub user_answer: Option<String>,
    #[serde(rename = "correctAnswer")]
    pub correct_answer: Option<String>,
    #[serde(rename = "isCorrect")]
    pub is_correct: Option<bool>,
    pub flagged: Option<bool>,
    pub subject: Option<String>,
    pub chapter: Option<String>,
    #[serde(rename = "knowledgePoint")]
    pub knowledge_point: Option<String>,
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<i64>,
    #[serde(rename = "submittedAt")]
    pub submitted_at: Option<i64>,
}

#[derive(Debug)]
pub struct AttemptRow {
    pub idempotency_key: String,
    pub session_key: String,
    pub device_id: String,
    pub bank_key: String,
    pub question_key: String,
    pub original_question_id: String,
    pub session_question_id: String,
    pub question_snapshot: String,
    pub user_answer: Option<String>,
    pub correct_answer: String,
    pub is_correct: Option<bool>,
    pub flagged: bool,
    pub subject: Option<String>,
    pub chapter: Option<String>,
    pub knowledge_point: Option<String>,
    pub duration_ms: i64,
    pub submitted_at: i64,
    pub submitted_at_explicit: bool,
}

#[derive(Debug, Serialize)]
pub struct AttemptOut {
    pub seq: i64,
    #[serde(rename = "idempotencyKey")]
    pub idempotency_key: String,
    #[serde(rename = "sessionKey")]
    pub session_key: String,
    #[serde(rename = "deviceId")]
    pub device_id: String,
    #[serde(rename = "bankKey")]
    pub bank_key: String,
    #[serde(rename = "questionKey")]
    pub question_key: String,
    #[serde(rename = "originalQuestionId")]
    pub original_question_id: String,
    #[serde(rename = "sessionQuestionId")]
    pub session_question_id: String,
    #[serde(rename = "questionSnapshot")]
    pub question_snapshot: serde_json::Value,
    #[serde(rename = "userAnswer")]
    pub user_answer: Option<String>,
    #[serde(rename = "correctAnswer")]
    pub correct_answer: String,
    #[serde(rename = "isCorrect")]
    pub is_correct: Option<bool>,
    pub flagged: bool,
    pub subject: Option<String>,
    pub chapter: Option<String>,
    #[serde(rename = "knowledgePoint")]
    pub knowledge_point: Option<String>,
    #[serde(rename = "durationMs")]
    pub duration_ms: i64,
    #[serde(rename = "submittedAt")]
    pub submitted_at: i64,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "feedSeq", skip_serializing_if = "Option::is_none")]
    pub feed_seq: Option<i64>,
    #[serde(rename = "wrongCount", skip_serializing_if = "Option::is_none")]
    pub wrong_count: Option<i64>,
    #[serde(rename = "attemptCount", skip_serializing_if = "Option::is_none")]
    pub attempt_count: Option<i64>,
    #[serde(skip)]
    pub snapshot_raw: String,
}

const ATTEMPT_COLUMNS: &str = "seq, idempotency_key, session_key, device_id, bank_key, question_key, original_question_id, session_question_id, question_snapshot, user_answer, user_answer_is_null, correct_answer, is_correct, flagged, subject, chapter, knowledge_point, duration_ms, submitted_at, created_at";
const ATTEMPT_COLUMNS_A: &str = "a.seq, a.idempotency_key, a.session_key, a.device_id, a.bank_key, a.question_key, a.original_question_id, a.session_question_id, a.question_snapshot, a.user_answer, a.user_answer_is_null, a.correct_answer, a.is_correct, a.flagged, a.subject, a.chapter, a.knowledge_point, a.duration_ms, a.submitted_at, a.created_at";

fn map_attempt(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttemptOut> {
    let snapshot_raw: String = row.get(8)?;
    let question_snapshot = if snapshot_raw.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&snapshot_raw).unwrap_or(serde_json::Value::Null)
    };
    let user_answer_raw: String = row.get(9)?;
    let user_answer_is_null = row.get::<_, i64>(10)? != 0;
    Ok(AttemptOut {
        seq: row.get(0)?,
        idempotency_key: row.get(1)?,
        session_key: row.get(2)?,
        device_id: row.get(3)?,
        bank_key: row.get(4)?,
        question_key: row.get(5)?,
        original_question_id: row.get(6)?,
        session_question_id: row.get(7)?,
        question_snapshot,
        user_answer: if user_answer_is_null { None } else { Some(user_answer_raw) },
        correct_answer: row.get(11)?,
        is_correct: row.get::<_, Option<i64>>(12)?.map(|v| v != 0),
        flagged: row.get::<_, i64>(13)? != 0,
        subject: row.get(14)?,
        chapter: row.get(15)?,
        knowledge_point: row.get(16)?,
        duration_ms: row.get(17)?,
        submitted_at: row.get(18)?,
        created_at: row.get(19)?,
        feed_seq: None,
        wrong_count: None,
        attempt_count: None,
        snapshot_raw,
    })
}

fn map_feed_attempt(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttemptOut> {
    let mut attempt = map_attempt(row)?;
    attempt.feed_seq = Some(row.get(20)?);
    attempt.wrong_count = Some(row.get(21)?);
    attempt.attempt_count = Some(row.get(22)?);
    Ok(attempt)
}

fn require_key(value: Option<String>, field: &str) -> Result<String, String> {
    let value = value.ok_or_else(|| format!("{field} is required"))?;
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if value.len() > MAX_KEY_LEN {
        return Err(format!("{field} exceeds {MAX_KEY_LEN} characters"));
    }
    Ok(value)
}

fn optional_text(value: Option<String>, field: &str, max: usize) -> Result<String, String> {
    match value {
        None => Ok(String::new()),
        Some(v) if v.len() <= max => Ok(v),
        Some(_) => Err(format!("{field} exceeds {max} characters")),
    }
}

fn optional_tag(value: Option<String>, field: &str) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) if v.len() > MAX_TAG_LEN => {
            Err(format!("{field} exceeds {MAX_TAG_LEN} characters"))
        }
        Some(v) => Ok(Some(v)),
    }
}

fn normalize_snapshot(value: Option<&serde_json::Value>) -> Result<String, String> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(String::new()),
        Some(v) => {
            let s = serde_json::to_string(v)
                .map_err(|e| format!("questionSnapshot is not serializable: {e}"))?;
            if s.len() > MAX_SNAPSHOT_BYTES {
                return Err(format!(
                    "questionSnapshot exceeds {MAX_SNAPSHOT_BYTES} bytes"
                ));
            }
            Ok(s)
        }
    }
}

fn normalize_attempt(input: &AttemptInput, now: i64) -> Result<AttemptRow, String> {
    let idempotency_key = require_key(input.idempotency_key.clone(), "idempotencyKey")?;
    let session_key = require_key(input.session_key.clone(), "sessionKey")?;
    let device_id = optional_text(input.device_id.clone(), "deviceId", MAX_KEY_LEN)?;
    let bank_key = optional_text(input.bank_key.clone(), "bankKey", MAX_KEY_LEN)?;
    let question_key = require_key(input.question_key.clone(), "questionKey")?;
    let original_question_id = optional_text(
        input.original_question_id.clone(),
        "originalQuestionId",
        MAX_KEY_LEN,
    )?;
    let session_question_id = optional_text(
        input.session_question_id.clone(),
        "sessionQuestionId",
        MAX_KEY_LEN,
    )?;
    let question_snapshot = normalize_snapshot(input.question_snapshot.as_ref())?;
    let user_answer = match input.user_answer.clone() {
        None => None,
        Some(value) if value.len() <= MAX_ANSWER_LEN => Some(value),
        Some(_) => return Err(format!("userAnswer exceeds {MAX_ANSWER_LEN} characters")),
    };
    let correct_answer = optional_text(
        input.correct_answer.clone(),
        "correctAnswer",
        MAX_ANSWER_LEN,
    )?;
    let subject = optional_tag(input.subject.clone(), "subject")?;
    let chapter = optional_tag(input.chapter.clone(), "chapter")?;
    let knowledge_point = optional_tag(input.knowledge_point.clone(), "knowledgePoint")?;
    let duration_ms = input.duration_ms.unwrap_or(0);
    if duration_ms < 0 {
        return Err("durationMs must not be negative".to_string());
    }
    let (submitted_at, submitted_at_explicit) = match input.submitted_at {
        Some(v) if v < 0 => return Err("submittedAt must not be negative".to_string()),
        Some(v) => (v, true),
        None => (now, false),
    };
    Ok(AttemptRow {
        idempotency_key,
        session_key,
        device_id,
        bank_key,
        question_key,
        original_question_id,
        session_question_id,
        question_snapshot,
        user_answer,
        correct_answer,
        is_correct: input.is_correct,
        flagged: input.flagged.unwrap_or(false),
        subject,
        chapter,
        knowledge_point,
        duration_ms,
        submitted_at,
        submitted_at_explicit,
    })
}

/// Immutable attempt identity/content must match for a retry with the same
/// idempotency key. `flagged` may toggle and `isCorrect` may transition once
/// from unknown to a final grade. `submittedAt` may be omitted by the client.
fn rows_match(existing: &AttemptOut, incoming: &AttemptRow) -> bool {
    if existing.session_key != incoming.session_key
        || existing.device_id != incoming.device_id
        || existing.bank_key != incoming.bank_key
        || existing.question_key != incoming.question_key
        || existing.original_question_id != incoming.original_question_id
        || existing.session_question_id != incoming.session_question_id
        || existing.snapshot_raw != incoming.question_snapshot
        || existing.user_answer != incoming.user_answer
        || existing.correct_answer != incoming.correct_answer
        || existing.subject != incoming.subject
        || existing.chapter != incoming.chapter
        || existing.knowledge_point != incoming.knowledge_point
        || existing.duration_ms != incoming.duration_ms
    {
        return false;
    }
    if incoming.submitted_at_explicit && existing.submitted_at != incoming.submitted_at {
        return false;
    }
    true
}

fn fetch_attempt_by_idempotency(
    conn: &Connection,
    key: &str,
) -> Result<Option<AttemptOut>, String> {
    let sql = format!("SELECT {ATTEMPT_COLUMNS} FROM study_attempts WHERE idempotency_key = ?1");
    conn.query_row(sql.as_str(), params![key], map_attempt)
        .optional()
        .map_err(|e| e.to_string())
}

struct BatchCounts {
    accepted: u64,
    updated: u64,
    duplicates: u64,
}

fn validate_batch(inputs: &[AttemptInput], now: i64) -> Result<Vec<AttemptRow>, String> {
    inputs
        .iter()
        .enumerate()
        .map(|(i, input)| normalize_attempt(input, now).map_err(|e| format!("attempts[{i}]: {e}")))
        .collect()
}

fn insert_review_event(
    conn: &Connection,
    attempt_seq: i64,
    reason: &str,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO study_review_events (attempt_seq, reason, created_at) VALUES (?1, ?2, ?3)",
        params![attempt_seq, reason, now],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn insert_batch(conn: &Connection, rows: &[AttemptRow], now: i64) -> Result<BatchCounts, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut counts = BatchCounts {
        accepted: 0,
        updated: 0,
        duplicates: 0,
    };
    let mut touched_sessions: Vec<String> = Vec::new();
    for row in rows {
        let inserted = tx
            .execute(
                "INSERT INTO study_attempts
                   (idempotency_key, session_key, device_id, bank_key, question_key, original_question_id,
                    session_question_id, question_snapshot, user_answer, user_answer_is_null, correct_answer, is_correct, flagged,
                    subject, chapter, knowledge_point, duration_ms, submitted_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
                 ON CONFLICT(idempotency_key) DO NOTHING",
                params![
                    row.idempotency_key,
                    row.session_key,
                    row.device_id,
                    row.bank_key,
                    row.question_key,
                    row.original_question_id,
                    row.session_question_id,
                    row.question_snapshot,
                    row.user_answer.as_deref().unwrap_or(""),
                    row.user_answer.is_none(),
                    row.correct_answer,
                    row.is_correct,
                    row.flagged,
                    row.subject,
                    row.chapter,
                    row.knowledge_point,
                    row.duration_ms,
                    row.submitted_at,
                    now,
                ],
            )
            .map_err(|e| e.to_string())?;
        if inserted > 0 {
            let attempt_seq = tx.last_insert_rowid();
            if row.is_correct == Some(false) || row.flagged {
                let reason = if row.is_correct == Some(false) {
                    "wrong"
                } else {
                    "flagged"
                };
                insert_review_event(&tx, attempt_seq, reason, now)?;
            }
            counts.accepted += 1;
            touched_sessions.push(row.session_key.clone());
            continue;
        }
        // Duplicate idempotency key: immutable content cannot be rewritten.
        // Two mutable transitions are allowed in place:
        //   1) flagged may toggle;
        //   2) an ungraded attempt may receive its first final grade.
        let existing = fetch_attempt_by_idempotency(&tx, &row.idempotency_key)?
            .ok_or_else(|| "inconsistent duplicate idempotency key".to_string())?;
        if !rows_match(&existing, row) {
            counts.duplicates += 1;
            continue;
        }
        let grade_changed = existing.is_correct != row.is_correct;
        if grade_changed && existing.is_correct.is_some() {
            // A final grade is immutable; do not silently rewrite history.
            counts.duplicates += 1;
            continue;
        }
        let flag_changed = existing.flagged != row.flagged;
        if grade_changed || flag_changed {
            tx.execute(
                "UPDATE study_attempts SET is_correct = ?1, flagged = ?2 WHERE seq = ?3",
                params![row.is_correct, row.flagged, existing.seq],
            )
            .map_err(|e| e.to_string())?;

            let became_wrong = existing.is_correct != Some(false) && row.is_correct == Some(false);
            let became_flagged = !existing.flagged && row.flagged;
            if became_wrong {
                insert_review_event(&tx, existing.seq, "wrong", now)?;
            } else if became_flagged {
                insert_review_event(&tx, existing.seq, "flagged", now)?;
            }
            counts.updated += 1;
            touched_sessions.push(row.session_key.clone());
        } else {
            counts.duplicates += 1;
        }
    }
    let mut seen = HashSet::new();
    for session_key in &touched_sessions {
        if seen.insert(session_key.as_str()) {
            upsert_session(&tx, session_key, None, None, now)?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(counts)
}

#[cfg(test)]
fn process_batch(
    conn: &Connection,
    inputs: &[AttemptInput],
    now: i64,
) -> Result<BatchCounts, String> {
    let rows = validate_batch(inputs, now)?;
    insert_batch(conn, &rows, now)
}

#[derive(Deserialize)]
pub struct BatchReq {
    #[serde(default)]
    pub attempts: Vec<AttemptInput>,
}

#[derive(Serialize)]
pub struct BatchRes {
    pub accepted: u64,
    pub updated: u64,
    pub duplicates: u64,
}

pub async fn attempts_batch_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchReq>,
) -> Result<Json<BatchRes>, Err> {
    if req.attempts.len() > MAX_BATCH_ATTEMPTS {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "too_many_attempts"));
    }
    let now = now_ms();
    let rows = validate_batch(&req.attempts, now)
        .map_err(|e| err_msg(StatusCode::BAD_REQUEST, "invalid_attempts", e))?;
    let conn = lock_conn(&state)?;
    let counts = insert_batch(&conn, &rows, now)
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    Ok(Json(BatchRes {
        accepted: counts.accepted,
        updated: counts.updated,
        duplicates: counts.duplicates,
    }))
}


#[derive(Deserialize)]
pub struct FlagReq {
    pub flagged: bool,
}

fn run_flag_update(
    conn: &Connection,
    idempotency_key: &str,
    flagged: bool,
    now: i64,
) -> Result<Option<AttemptOut>, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let Some(existing) = fetch_attempt_by_idempotency(&tx, idempotency_key)? else {
        tx.commit().map_err(|e| e.to_string())?;
        return Ok(None);
    };
    if existing.flagged != flagged {
        tx.execute(
            "UPDATE study_attempts SET flagged = ?1 WHERE seq = ?2",
            params![flagged, existing.seq],
        )
        .map_err(|e| e.to_string())?;
        if !existing.flagged && flagged {
            insert_review_event(&tx, existing.seq, "flagged", now)?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    fetch_attempt_by_idempotency(conn, idempotency_key)
}

pub async fn attempt_flag_handler(
    State(state): State<Arc<AppState>>,
    Path(idempotency_key): Path<String>,
    Json(req): Json<FlagReq>,
) -> Result<Json<serde_json::Value>, Err> {
    if idempotency_key.trim().is_empty() || idempotency_key.len() > MAX_KEY_LEN {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_attempt_id"));
    }
    let conn = lock_conn(&state)?;
    let attempt = run_flag_update(&conn, &idempotency_key, req.flagged, now_ms())
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "attempt_not_found"))?;
    Ok(Json(serde_json::json!({
        "idempotencyKey": attempt.idempotency_key,
        "flagged": attempt.flagged,
    })))
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// Upsert a session row. `started_at` may be `None` (then it is inferred from
/// the earliest stored attempt); both timestamps only ever merge monotonically
/// (earliest started_at, latest finished_at) so out-of-order uploads can never
/// move a session backwards.
fn upsert_session(
    conn: &Connection,
    session_key: &str,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO study_sessions (session_key, started_at, finished_at, updated_at)
         VALUES (?1, COALESCE(?2, (SELECT MIN(submitted_at) FROM study_attempts WHERE session_key = ?1)), ?3, ?4)
         ON CONFLICT(session_key) DO UPDATE SET
           started_at = CASE
             WHEN excluded.started_at IS NULL THEN study_sessions.started_at
             WHEN study_sessions.started_at IS NULL THEN excluded.started_at
             ELSE MIN(study_sessions.started_at, excluded.started_at)
           END,
           finished_at = CASE
             WHEN excluded.finished_at IS NULL THEN study_sessions.finished_at
             WHEN study_sessions.finished_at IS NULL THEN excluded.finished_at
             ELSE MAX(study_sessions.finished_at, excluded.finished_at)
           END,
           updated_at = excluded.updated_at",
        params![session_key, started_at, finished_at, now],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn fetch_session_row(
    conn: &Connection,
    session_key: &str,
) -> Result<(Option<i64>, Option<i64>), String> {
    conn.query_row(
        "SELECT started_at, finished_at FROM study_sessions WHERE session_key = ?1",
        params![session_key],
        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("session row missing for {session_key}"))
}

#[derive(Deserialize)]
pub struct FinishReq {
    #[serde(rename = "sessionKey")]
    pub session_key: Option<String>,
    #[serde(rename = "finishedAt")]
    pub finished_at: Option<i64>,
    #[serde(rename = "startedAt")]
    pub started_at: Option<i64>,
}

pub async fn sessions_finish_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FinishReq>,
) -> Result<Json<serde_json::Value>, Err> {
    let session_key = require_key(req.session_key, "sessionKey")
        .map_err(|e| err_msg(StatusCode::BAD_REQUEST, "invalid_session", e))?;
    let finished_at = req.finished_at.ok_or_else(|| {
        err_msg(
            StatusCode::BAD_REQUEST,
            "invalid_session",
            "finishedAt is required",
        )
    })?;
    if finished_at < 0 {
        return Err(err_msg(
            StatusCode::BAD_REQUEST,
            "invalid_session",
            "finishedAt must not be negative",
        ));
    }
    if let Some(started) = req.started_at {
        if started < 0 {
            return Err(err_msg(
                StatusCode::BAD_REQUEST,
                "invalid_session",
                "startedAt must not be negative",
            ));
        }
    }
    let conn = lock_conn(&state)?;
    let now = now_ms();
    upsert_session(&conn, &session_key, req.started_at, Some(finished_at), now)
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    let (started_at, stored_finished_at) = fetch_session_row(&conn, &session_key)
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    Ok(Json(serde_json::json!({
        "sessionKey": session_key,
        "startedAt": started_at,
        "finishedAt": stored_finished_at,
    })))
}


pub async fn sessions_finish_path_handler(
    State(state): State<Arc<AppState>>,
    Path(session_key): Path<String>,
    Json(mut req): Json<FinishReq>,
) -> Result<Json<serde_json::Value>, Err> {
    if let Some(body_key) = req.session_key.as_deref() {
        if body_key != session_key {
            return Err(err_msg(
                StatusCode::BAD_REQUEST,
                "invalid_session",
                "sessionKey in body must match path",
            ));
        }
    }
    req.session_key = Some(session_key);
    sessions_finish_handler(State(state), Json(req)).await
}

struct LatestSession {
    session_key: String,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    total: i64,
    correct: i64,
    wrong: i64,
    flagged: i64,
}

fn run_latest(conn: &Connection) -> Result<Option<LatestSession>, Err> {
    conn.query_row(
        "SELECT s.session_key, s.started_at, s.finished_at,
                (SELECT COUNT(*) FROM study_attempts a WHERE a.session_key = s.session_key),
                (SELECT COUNT(*) FROM study_attempts a WHERE a.session_key = s.session_key AND a.is_correct = 1),
                (SELECT COUNT(*) FROM study_attempts a WHERE a.session_key = s.session_key AND a.is_correct = 0),
                (SELECT COUNT(*) FROM study_attempts a WHERE a.session_key = s.session_key AND a.flagged = 1)
         FROM study_sessions s
         ORDER BY COALESCE(s.started_at, 0) DESC, s.updated_at DESC, s.session_key ASC
         LIMIT 1",
        [],
        |r| {
            Ok(LatestSession {
                session_key: r.get(0)?,
                started_at: r.get(1)?,
                finished_at: r.get(2)?,
                total: r.get(3)?,
                correct: r.get(4)?,
                wrong: r.get(5)?,
                flagged: r.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))
}

pub async fn sessions_latest_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, Err> {
    let conn = lock_conn(&state)?;
    let Some(latest) = run_latest(&conn)? else {
        return Err(err(StatusCode::NOT_FOUND, "no_sessions"));
    };
    Ok(Json(serde_json::json!({
        "sessionKey": latest.session_key,
        "startedAt": latest.started_at,
        "finishedAt": latest.finished_at,
        "stats": {
            "total": latest.total,
            "correct": latest.correct,
            "wrong": latest.wrong,
            "flagged": latest.flagged,
        },
    })))
}

// ---------------------------------------------------------------------------
// Feed (wrong/flagged attempts) with per-consumer cursors
// ---------------------------------------------------------------------------

struct FeedPage {
    stored_cursor: i64,
    next_cursor: i64,
    attempts: Vec<AttemptOut>,
}

fn run_feed(
    conn: &Connection,
    consumer: &str,
    after: Option<i64>,
    subject: Option<&str>,
    chapter: Option<&str>,
    bank_key: Option<&str>,
    include_flagged: bool,
    limit: i64,
) -> Result<FeedPage, Err> {
    let stored: i64 = conn
        .query_row(
            "SELECT cursor FROM study_consumers WHERE consumer = ?1",
            params![consumer],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?
        .unwrap_or(0);
    let start = after.unwrap_or(stored);
    let sql = format!(
        "SELECT {ATTEMPT_COLUMNS_A}, MAX(e.seq) AS feed_seq,
                (SELECT COUNT(*) FROM study_attempts aw
                 WHERE aw.question_key = a.question_key AND aw.is_correct = 0) AS wrong_count,
                (SELECT COUNT(*) FROM study_attempts aa
                 WHERE aa.question_key = a.question_key) AS attempt_count
         FROM study_review_events e
         JOIN study_attempts a ON a.seq = e.attempt_seq
         WHERE e.seq > ?1
           AND (a.is_correct = 0 OR (?5 != 0 AND a.flagged = 1))
           AND (?2 IS NULL OR a.subject = ?2 COLLATE NOCASE)
           AND (?3 IS NULL OR a.chapter = ?3 COLLATE NOCASE)
           AND (?4 IS NULL OR a.bank_key = ?4)
         GROUP BY a.seq
         ORDER BY feed_seq ASC LIMIT ?6"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    let attempts: Vec<AttemptOut> = stmt
        .query_map(
            params![
                start,
                subject,
                chapter,
                bank_key,
                include_flagged as i64,
                limit
            ],
            map_feed_attempt,
        )
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?
        .filter_map(|r| r.ok())
        .collect();
    let next_cursor = attempts.last().and_then(|a| a.feed_seq).unwrap_or(start);
    Ok(FeedPage {
        stored_cursor: start,
        next_cursor,
        attempts,
    })
}

fn run_ack(conn: &Connection, consumer: &str, cursor: i64) -> Result<i64, Err> {
    // AUTOINCREMENT high-water is monotonic even if review rows are ever
    // deleted/compacted later; MAX(seq) could move backwards after deletion.
    let max_seq: i64 = conn
        .query_row(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'study_review_events'), 0)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    if cursor > max_seq {
        return Err(err_msg(
            StatusCode::BAD_REQUEST,
            "CURSOR_AHEAD_OF_FEED",
            format!("cursor {cursor} is ahead of the latest seq {max_seq}"),
        ));
    }
    conn.execute(
        "INSERT INTO study_consumers (consumer, cursor, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(consumer) DO UPDATE SET
           cursor = MAX(study_consumers.cursor, excluded.cursor),
           updated_at = excluded.updated_at",
        params![consumer, cursor, now_ms()],
    )
    .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    conn.query_row(
        "SELECT cursor FROM study_consumers WHERE consumer = ?1",
        params![consumer],
        |r| r.get(0),
    )
    .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))
}

#[derive(Deserialize)]
pub struct FeedQuery {
    pub consumer: Option<String>,
    pub after: Option<i64>,
    pub subject: Option<String>,
    pub chapter: Option<String>,
    #[serde(rename = "bankKey")]
    pub bank_key: Option<String>,
    #[serde(rename = "includeFlagged")]
    pub include_flagged: Option<bool>,
    pub limit: Option<i64>,
}

fn consumer_key(value: Option<String>) -> Result<String, Err> {
    let consumer = require_key(value, "consumer")
        .map_err(|e| err_msg(StatusCode::BAD_REQUEST, "invalid_query", e))?;
    if consumer.len() > MAX_CONSUMER_LEN {
        return Err(err_msg(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            format!("consumer exceeds {MAX_CONSUMER_LEN} characters"),
        ));
    }
    Ok(consumer)
}

fn tag_filter(value: Option<String>, field: &str) -> Result<Option<String>, Err> {
    match value {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) if v.len() > MAX_TAG_LEN => Err(err_msg(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            format!("{field} exceeds {MAX_TAG_LEN} characters"),
        )),
        Some(v) => Ok(Some(v)),
    }
}

pub async fn feed_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FeedQuery>,
) -> Result<Json<serde_json::Value>, Err> {
    let consumer = consumer_key(q.consumer)?;
    if let Some(after) = q.after {
        if after < 0 {
            return Err(err_msg(
                StatusCode::BAD_REQUEST,
                "invalid_query",
                "after must not be negative",
            ));
        }
    }
    let subject = tag_filter(q.subject, "subject")?;
    let chapter = tag_filter(q.chapter, "chapter")?;
    let bank_key = tag_filter(q.bank_key, "bankKey")?;
    let include_flagged = q.include_flagged.unwrap_or(true);
    let limit = q
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .clamp(1, MAX_PAGE_LIMIT);
    let conn = lock_conn(&state)?;
    let page = run_feed(
        &conn,
        &consumer,
        q.after,
        subject.as_deref(),
        chapter.as_deref(),
        bank_key.as_deref(),
        include_flagged,
        limit,
    )?;
    Ok(Json(serde_json::json!({
        "attempts": page.attempts,
        "storedCursor": page.stored_cursor,
        "nextCursor": page.next_cursor,
    })))
}

#[derive(Deserialize)]
pub struct AckReq {
    pub consumer: Option<String>,
    pub cursor: Option<i64>,
}

pub async fn feed_ack_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AckReq>,
) -> Result<Json<serde_json::Value>, Err> {
    let consumer = require_key(req.consumer, "consumer")
        .map_err(|e| err_msg(StatusCode::BAD_REQUEST, "invalid_ack", e))?;
    if consumer.len() > MAX_CONSUMER_LEN {
        return Err(err_msg(
            StatusCode::BAD_REQUEST,
            "invalid_ack",
            format!("consumer exceeds {MAX_CONSUMER_LEN} characters"),
        ));
    }
    let cursor = req
        .cursor
        .ok_or_else(|| err_msg(StatusCode::BAD_REQUEST, "invalid_ack", "cursor is required"))?;
    if cursor < 0 {
        return Err(err_msg(
            StatusCode::BAD_REQUEST,
            "invalid_ack",
            "cursor must not be negative",
        ));
    }
    let conn = lock_conn(&state)?;
    let stored = run_ack(&conn, &consumer, cursor)?;
    Ok(Json(serde_json::json!({
        "consumer": consumer,
        "cursor": stored,
    })))
}

// ---------------------------------------------------------------------------
// Question history
// ---------------------------------------------------------------------------

fn run_history(conn: &Connection, question_key: &str, limit: i64) -> Result<Vec<AttemptOut>, Err> {
    let sql = format!(
        "SELECT {ATTEMPT_COLUMNS} FROM study_attempts
         WHERE question_key = ?1 ORDER BY seq DESC LIMIT ?2"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    let attempts: Vec<AttemptOut> = stmt
        .query_map(params![question_key, limit], map_attempt)
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(attempts)
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<i64>,
}

pub async fn question_history_handler(
    State(state): State<Arc<AppState>>,
    Path(question_key): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, Err> {
    if question_key.trim().is_empty() || question_key.len() > MAX_KEY_LEN {
        return Err(err_msg(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            format!("question key exceeds {MAX_KEY_LEN} characters"),
        ));
    }
    let limit = q
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .clamp(1, MAX_PAGE_LIMIT);
    let conn = lock_conn(&state)?;
    let attempts = run_history(&conn, &question_key, limit)?;
    Ok(Json(serde_json::json!({
        "questionKey": question_key,
        "attempts": attempts,
    })))
}

// ---------------------------------------------------------------------------
// Banks
// ---------------------------------------------------------------------------

/// Study-bank schema version accepted by `validate_bank`
/// (mirrors `SCHEMA_VERSION` in tools/exameowctl/schema.py).
const BANK_SCHEMA_VERSION: i64 = 1;
/// schema.py `MAX_KEY_LENGTH`: bank keys and question stableKeys are
/// identifiers of at most 128 characters (deliberately stricter than the
/// `MAX_KEY_LEN` used for attempt/session keys).
const MAX_SCHEMA_KEY_LEN: usize = 128;
const MIN_SCHEMA_KEY_LEN: usize = 3;
const BANK_METADATA_FIELDS: &[&str] = &["exam", "outlineVersion", "subject"];
/// schema.py `QUESTION_TYPES`: `short_answer` is deliberately not allowed.
const BANK_QUESTION_TYPES: &[&str] = &["single_choice", "multi_choice", "true_false", "fill_blank"];
const BANK_CHOICE_TYPES: &[&str] = &["single_choice", "multi_choice"];
const BANK_DIFFICULTIES: &[&str] = &["easy", "medium", "hard"];
const BANK_FIELDS: &[&str] = &[
    "schemaVersion",
    "key",
    "name",
    "version",
    "metadata",
    "questions",
    "subject",
    "chapter",
    "tags",
    "sourceMeta",
];
const BANK_QUESTION_FIELDS: &[&str] = &[
    "id",
    "stableKey",
    "type",
    "stem",
    "options",
    "answer",
    "analysis",
    "subject",
    "chapter",
    "knowledgePoint",
    "difficulty",
    "tags",
    "sourceMeta",
];

/// Free-text field check (schema.py `_check_text`): a non-empty, non
/// whitespace-only string.
fn check_text(value: &serde_json::Value, path: &str) -> Result<(), String> {
    match value.as_str() {
        None => Err(format!("{path} must be a string")),
        Some(s) if s.trim().is_empty() => {
            Err(format!("{path} must not be empty or whitespace-only"))
        }
        Some(_) => Ok(()),
    }
}

/// Identifier check for machine-facing keys, i.e. bank keys and stableKeys
/// (schema.py `_check_identifier`): non-empty, no whitespace, no `/` or `\`,
/// no control characters, at most `MAX_SCHEMA_KEY_LEN` characters.
fn check_identifier(value: &serde_json::Value, path: &str) -> Result<(), String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{path} must be a string"))?;
    if text.trim().is_empty() {
        return Err(format!("{path} must not be empty"));
    }
    let char_count = text.chars().count();
    if char_count < MIN_SCHEMA_KEY_LEN {
        return Err(format!(
            "{path} must be at least {MIN_SCHEMA_KEY_LEN} characters"
        ));
    }
    if char_count > MAX_SCHEMA_KEY_LEN {
        return Err(format!(
            "{path} must be at most {MAX_SCHEMA_KEY_LEN} characters (got {char_count})"
        ));
    }
    if text.chars().any(char::is_whitespace) {
        return Err(format!("{path} must not contain whitespace"));
    }
    if text.contains('/') || text.contains('\\') {
        return Err(format!("{path} must not contain '/' or '\\'"));
    }
    if text.chars().any(|c| (c as u32) < 32 || (c as u32) == 127) {
        return Err(format!("{path} must not contain control characters"));
    }
    if !text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
    {
        return Err(format!(
            "{path} may contain only ASCII letters, digits, '.', '_', ':', or '-'"
        ));
    }
    Ok(())
}

/// schema.py `_check_tags`: a list of non-empty strings.
fn check_metadata(value: &serde_json::Value) -> Result<(), String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "metadata must be an object".to_string())?;
    check_unknown_fields(obj, BANK_METADATA_FIELDS, "metadata")?;
    for field in BANK_METADATA_FIELDS {
        let value = obj
            .get(*field)
            .ok_or_else(|| format!("metadata: missing required metadata field: {field}"))?;
        check_text(value, &format!("metadata.{field}"))?;
    }
    Ok(())
}

fn check_tags(value: &serde_json::Value, path: &str) -> Result<(), String> {
    let tags = value
        .as_array()
        .ok_or_else(|| format!("{path} must be a list of strings"))?;
    let mut seen = HashSet::new();
    for (tag_index, tag) in tags.iter().enumerate() {
        check_text(tag, &format!("{path}[{tag_index}]"))?;
        if let Some(text) = tag.as_str() {
            if !seen.insert(text) {
                return Err(format!(
                    "{path}: duplicate tag '{text}'; tags must be unique"
                ));
            }
        }
    }
    Ok(())
}

/// schema.py `_check_unknown_fields`: reject fields outside `allowed` so
/// typos fail loudly instead of being silently dropped.
fn check_unknown_fields(
    obj: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    path: &str,
) -> Result<(), String> {
    let mut extra: Vec<&str> = obj
        .keys()
        .filter(|field| !allowed.contains(&field.as_str()))
        .map(|field| field.as_str())
        .collect();
    if extra.is_empty() {
        return Ok(());
    }
    extra.sort_unstable();
    Err(format!(
        "{path}: unknown field(s): {} (allowed: {})",
        extra.join(", "),
        allowed.join(", ")
    ))
}

/// schema.py `_check_choice_answer`: a non-empty string of distinct uppercase
/// option letters; `single_choice` has exactly one letter; every letter must
/// exist among the options (cross-checked only when the option count is
/// valid, matching schema.py).
fn check_choice_answer(
    value: &serde_json::Value,
    path: &str,
    option_count: usize,
    multi: bool,
) -> Result<(), String> {
    let answer = value.as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
        format!(
            "{path}: answer must be a non-empty string of option letters (e.g. \"A\" or \"ABD\")"
        )
    })?;
    let letters: Vec<char> = answer.chars().collect();
    if letters.iter().any(|c| !c.is_ascii_uppercase()) {
        return Err(format!(
            "{path}: answer letters must be uppercase A-Z option letters (e.g. \"A\" or \"ABD\"); \
             lowercase or other characters are not accepted"
        ));
    }
    let mut seen = HashSet::new();
    if letters.iter().any(|c| !seen.insert(*c)) {
        return Err(format!("{path}: answer must not repeat option letters"));
    }
    if !multi && letters.len() != 1 {
        return Err(format!(
            "{path}: single_choice answer must be exactly one option letter (got \"{answer}\")"
        ));
    }
    if (2..=5).contains(&option_count) {
        let allowed: String = (0..option_count)
            .map(|i| char::from(b'A' + i as u8))
            .collect();
        let mut invalid: Vec<char> = letters
            .iter()
            .copied()
            .filter(|c| !allowed.contains(*c))
            .collect();
        invalid.sort_unstable();
        invalid.dedup();
        if !invalid.is_empty() {
            return Err(format!(
                "{path}: answer references option letter(s) '{}' that do not exist \
                 (options are {allowed})",
                invalid.into_iter().collect::<String>()
            ));
        }
    }
    Ok(())
}

/// Validate one bank question (schema.py `_validate_question`) with
/// first-error semantics; `index` is used in error paths.
fn validate_question(question: &serde_json::Value, index: usize) -> Result<(), String> {
    let path = format!("questions[{index}]");
    let obj = question
        .as_object()
        .ok_or_else(|| format!("{path}: question must be a JSON object"))?;
    check_unknown_fields(obj, BANK_QUESTION_FIELDS, &path)?;

    let question_type = match obj.get("type") {
        None => return Err(format!("{path}: missing required field: type")),
        Some(value) => {
            let question_type = value.as_str().ok_or_else(|| {
                format!(
                    "{path}.type: unknown question type (allowed: {})",
                    BANK_QUESTION_TYPES.join(", ")
                )
            })?;
            if !BANK_QUESTION_TYPES.contains(&question_type) {
                if question_type == "short_answer" {
                    return Err(format!(
                        "{path}.type: question type 'short_answer' is not allowed in study \
                         banks (allowed: {})",
                        BANK_QUESTION_TYPES.join(", ")
                    ));
                }
                return Err(format!(
                    "{path}.type: unknown question type '{question_type}' (allowed: {})",
                    BANK_QUESTION_TYPES.join(", ")
                ));
            }
            question_type
        }
    };

    for field in ["stem", "analysis"] {
        match obj.get(field) {
            None => return Err(format!("{path}: missing required field: {field}")),
            Some(value) => check_text(value, &format!("{path}.{field}"))?,
        }
    }

    let has_id = obj.contains_key("id");
    let has_stable_key = obj.contains_key("stableKey");
    if !has_id && !has_stable_key {
        return Err(format!(
            "{path}: missing required field: 'id' or 'stableKey' (at least one)"
        ));
    }
    if let Some(value) = obj.get("id") {
        check_identifier(value, &format!("{path}.id"))?;
    }
    if let Some(value) = obj.get("stableKey") {
        check_identifier(value, &format!("{path}.stableKey"))?;
    }

    let is_choice = matches!(question_type, "single_choice" | "multi_choice");
    let option_count: Option<usize> = if is_choice {
        let options = obj.get("options").ok_or_else(|| {
            format!(
                "{path}: missing required field: options ({question_type} questions must \
                 have 2-5 options)"
            )
        })?;
        let options = options
            .as_array()
            .ok_or_else(|| format!("{path}.options: options must be a list of strings"))?;
        let count = options.len();
        if !(2..=5).contains(&count) {
            return Err(format!(
                "{path}.options: choice questions must have 2-5 options (got {count})"
            ));
        }
        for (option_index, option) in options.iter().enumerate() {
            check_text(option, &format!("{path}.options[{option_index}]"))?;
        }
        Some(count)
    } else if obj.contains_key("options") {
        return Err(format!(
            "{path}.options: options are only allowed for {} questions",
            BANK_CHOICE_TYPES.join("/")
        ));
    } else {
        None
    };

    let answer = obj
        .get("answer")
        .ok_or_else(|| format!("{path}: missing required field: answer"))?;
    if is_choice {
        check_choice_answer(
            answer,
            &format!("{path}.answer"),
            option_count.unwrap_or(0),
            question_type == "multi_choice",
        )?;
    } else if question_type == "true_false" {
        if !matches!(answer.as_str(), Some("true") | Some("false")) {
            return Err(format!(
                "{path}.answer: true_false answer must be exactly \"true\" or \"false\""
            ));
        }
    } else {
        // fill_blank
        check_text(answer, &format!("{path}.answer"))?;
    }

    for field in ["subject", "chapter", "knowledgePoint"] {
        if let Some(value) = obj.get(field) {
            check_text(value, &format!("{path}.{field}"))?;
        }
    }
    if let Some(value) = obj.get("difficulty") {
        let valid = value
            .as_str()
            .is_some_and(|difficulty| BANK_DIFFICULTIES.contains(&difficulty));
        if !valid {
            return Err(format!(
                "{path}.difficulty: difficulty must be one of: {}",
                BANK_DIFFICULTIES.join(", ")
            ));
        }
    }
    if let Some(value) = obj.get("tags") {
        check_tags(value, &format!("{path}.tags"))?;
    }
    if let Some(value) = obj.get("sourceMeta") {
        if !value.is_object() {
            return Err(format!("{path}.sourceMeta: sourceMeta must be an object"));
        }
    }
    Ok(())
}

/// Validate a schemaVersion 1 bank object and return
/// `(key, name, payload, question_count)`. Mirrors `validate_bank` in
/// tools/exameowctl/schema.py with first-error semantics, plus the server's
/// own size/count safeguards.
fn validate_bank_v1(
    bank: &serde_json::Value,
) -> Result<(String, String, serde_json::Value, usize), String> {
    let obj = bank
        .as_object()
        .ok_or_else(|| "bank must be a JSON object".to_string())?;
    check_unknown_fields(obj, BANK_FIELDS, "(bank)")?;

    let version = match obj.get("schemaVersion") {
        None => {
            return Err(format!(
                "missing required field: schemaVersion (must be {BANK_SCHEMA_VERSION})"
            ));
        }
        Some(serde_json::Value::Number(number)) if number.is_i64() => number.as_i64().unwrap_or(0),
        Some(_) => {
            return Err(format!(
                "schemaVersion must be the integer {BANK_SCHEMA_VERSION}"
            ));
        }
    };
    if version != BANK_SCHEMA_VERSION {
        return Err(format!(
            "unsupported schemaVersion {version} (this server supports version \
             {BANK_SCHEMA_VERSION} only)"
        ));
    }

    let key_value = obj
        .get("key")
        .ok_or_else(|| "missing required field: key".to_string())?;
    check_identifier(key_value, "key")?;
    let bank_key = key_value.as_str().unwrap_or_default().to_string();

    let name_value = obj
        .get("name")
        .ok_or_else(|| "missing required field: name".to_string())?;
    check_text(name_value, "name")?;
    let name = name_value.as_str().unwrap_or_default().trim().to_string();
    if name.len() > MAX_BANK_NAME_LEN {
        return Err(format!("bank name exceeds {MAX_BANK_NAME_LEN} characters"));
    }

    let content_version = obj
        .get("version")
        .ok_or_else(|| "missing required field: version".to_string())?;
    let content_version = content_version
        .as_i64()
        .filter(|version| *version >= 1)
        .ok_or_else(|| "version must be an integer >= 1".to_string())?;
    let _ = content_version;

    let metadata = obj
        .get("metadata")
        .ok_or_else(|| "missing required field: metadata".to_string())?;
    check_metadata(metadata)?;

    let questions = obj
        .get("questions")
        .ok_or_else(|| "missing required field: questions".to_string())?
        .as_array()
        .ok_or_else(|| "questions must be a list".to_string())?;
    if questions.len() > MAX_BANK_QUESTIONS {
        return Err(format!("questions exceed {MAX_BANK_QUESTIONS} items"));
    }

    // Both local ids and stable keys must be unique within one bank. Local ids
    // are used by wrong-book/session state; stable keys are used for durable sync.
    let mut ids: HashSet<&str> = HashSet::new();
    let mut stable_keys: HashSet<&str> = HashSet::new();
    for (index, question) in questions.iter().enumerate() {
        validate_question(question, index)?;
        if let Some(obj) = question.as_object() {
            if let Some(id) = obj.get("id").and_then(|value| value.as_str()) {
                if !id.is_empty() && !ids.insert(id) {
                    return Err(format!(
                        "questions: duplicate id '{id}' (id values must be unique within one bank)"
                    ));
                }
            }
            if let Some(stable_key) = obj.get("stableKey").and_then(|value| value.as_str()) {
                if !stable_key.is_empty() && !stable_keys.insert(stable_key) {
                    return Err(format!(
                        "questions: duplicate stableKey '{stable_key}' (stableKey values must be unique within one bank)"
                    ));
                }
            }
        }
    }

    for field in ["subject", "chapter"] {
        if let Some(value) = obj.get(field) {
            check_text(value, field)?;
        }
    }
    if let Some(value) = obj.get("tags") {
        check_tags(value, "tags")?;
    }
    if let Some(value) = obj.get("sourceMeta") {
        if !value.is_object() {
            return Err("sourceMeta must be an object".to_string());
        }
    }

    let serialized = serde_json::to_string(bank)
        .map_err(|e| format!("bank payload is not serializable: {e}"))?;
    if serialized.len() > MAX_BANK_PAYLOAD_BYTES {
        return Err(format!(
            "bank payload exceeds {MAX_BANK_PAYLOAD_BYTES} bytes"
        ));
    }

    Ok((bank_key, name, bank.clone(), questions.len()))
}

/// Validate an import payload and return (bankKey, name, payload, questionCount).
///
/// schemaVersion 1 contract (tools/exameowctl/schema.py): the CLI posts the
/// bank object itself, i.e. the body *is* the bank (`{schemaVersion, key,
/// name, questions, ...}`). A legacy wrapper `{"bankKey": ..., "bank": {...}}`
/// is still accepted, but only when the nested bank is itself a compliant
/// schemaVersion 1 bank with its own `key`, and a wrapper `bankKey` (when
/// present) must match that nested `key`.
fn validate_bank(
    body: &serde_json::Value,
) -> Result<(String, String, serde_json::Value, usize), String> {
    let obj = body
        .as_object()
        .ok_or_else(|| "bank must be a JSON object".to_string())?;
    let (bank, wrapper_bank_key) = match obj.get("bank") {
        Some(nested) if nested.is_object() => {
            // The wrapper is only a transport envelope: `bank` plus an
            // optional `bankKey`. Anything else is a typo or a stale client.
            check_unknown_fields(obj, &["bank", "bankKey"], "(wrapper)")?;
            (nested, obj.get("bankKey"))
        }
        Some(_) => return Err("bank must be a JSON object".to_string()),
        None => (body, None),
    };

    let (bank_key, name, payload, question_count) = validate_bank_v1(bank)?;

    if let Some(wrapper_key) = wrapper_bank_key {
        let wrapper_key = wrapper_key
            .as_str()
            .ok_or_else(|| "bankKey must be a string".to_string())?;
        if wrapper_key != bank_key.as_str() {
            return Err(format!(
                "bankKey '{wrapper_key}' does not match nested bank key '{bank_key}'"
            ));
        }
    }

    Ok((bank_key, name, payload, question_count))
}

struct StoredBank {
    name: String,
    payload: String,
    content_hash: String,
    question_count: i64,
    created_at: i64,
    updated_at: i64,
}

fn run_bank_import(
    conn: &Connection,
    bank_key: &str,
    name: &str,
    payload_json: &str,
    question_count: usize,
    now: i64,
) -> Result<(), Err> {
    let content_hash = sha256_hex(payload_json);
    conn.execute(
        "INSERT INTO study_banks
           (bank_key, name, payload, content_hash, question_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(bank_key) DO UPDATE SET
           name = excluded.name,
           payload = excluded.payload,
           content_hash = excluded.content_hash,
           question_count = excluded.question_count,
           updated_at = CASE
             WHEN study_banks.content_hash = excluded.content_hash THEN study_banks.updated_at
             ELSE excluded.updated_at
           END",
        params![
            bank_key,
            name,
            payload_json,
            content_hash,
            question_count as i64,
            now
        ],
    )
    .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    Ok(())
}

fn run_bank_get(conn: &Connection, bank_key: &str) -> Result<Option<StoredBank>, Err> {
    conn.query_row(
        "SELECT name, payload, content_hash, question_count, created_at, updated_at FROM study_banks WHERE bank_key = ?1",
        params![bank_key],
        |r| {
            Ok(StoredBank {
                name: r.get(0)?,
                payload: r.get(1)?,
                content_hash: r.get(2)?,
                question_count: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))
}

pub async fn banks_import_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, Err> {
    let (bank_key, name, payload, question_count) =
        validate_bank(&body).map_err(|e| err_msg(StatusCode::BAD_REQUEST, "invalid_bank", e))?;
    let payload_json = serde_json::to_string(&payload)
        .map_err(|e| err_msg(StatusCode::BAD_REQUEST, "invalid_bank", e))?;
    let now = now_ms();
    let conn = lock_conn(&state)?;
    run_bank_import(&conn, &bank_key, &name, &payload_json, question_count, now)?;
    let stored = run_bank_get(&conn, &bank_key)?
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "bank_store_missing"))?;
    Ok(Json(serde_json::json!({
        "bankKey": bank_key,
        "contentHash": stored.content_hash,
        "questionCount": question_count,
        "updatedAt": stored.updated_at,
    })))
}

pub async fn banks_list_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, Err> {
    let conn = lock_conn(&state)?;
    let mut stmt = conn
        .prepare(
            "SELECT bank_key, name, content_hash, question_count, created_at, updated_at
             FROM study_banks ORDER BY updated_at DESC, bank_key ASC LIMIT ?1",
        )
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    let banks: Vec<serde_json::Value> = stmt
        .query_map(params![MAX_BANKS_LISTED], |r| {
            Ok(serde_json::json!({
                "bankKey": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "contentHash": r.get::<_, String>(2)?,
                "questionCount": r.get::<_, i64>(3)?,
                "createdAt": r.get::<_, i64>(4)?,
                "updatedAt": r.get::<_, i64>(5)?,
            }))
        })
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(Json(serde_json::json!({ "banks": banks })))
}

pub async fn banks_get_handler(
    State(state): State<Arc<AppState>>,
    Path(bank_key): Path<String>,
) -> Result<Json<serde_json::Value>, Err> {
    if bank_key.trim().is_empty() || bank_key.len() > MAX_KEY_LEN {
        return Err(err_msg(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            format!("bank key exceeds {MAX_KEY_LEN} characters"),
        ));
    }
    let conn = lock_conn(&state)?;
    let Some(stored) = run_bank_get(&conn, &bank_key)? else {
        return Err(err(StatusCode::NOT_FOUND, "bank_not_found"));
    };
    let bank: serde_json::Value = serde_json::from_str(&stored.payload)
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    Ok(Json(serde_json::json!({
        "bankKey": bank_key,
        "name": stored.name,
        "contentHash": stored.content_hash,
        "questionCount": stored.question_count,
        "createdAt": stored.created_at,
        "updatedAt": stored.updated_at,
        "bank": bank,
    })))
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

pub async fn health_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, Err> {
    let enabled = !state.study_token.trim().is_empty();
    if !enabled {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "study_sync_disabled"));
    }
    let conn = lock_conn(&state)?;
    conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "enabled": true,
        "authenticated": true,
    })))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_tables(&conn).unwrap();
        conn
    }

    fn attempt_json(
        idem: &str,
        session: &str,
        qkey: &str,
        is_correct: serde_json::Value,
        flagged: bool,
        subject: &str,
        submitted_at: i64,
    ) -> serde_json::Value {
        json!({
            "idempotencyKey": idem,
            "sessionKey": session,
            "deviceId": "test-device",
            "bankKey": "bank-main",
            "questionKey": qkey,
            "originalQuestionId": qkey,
            "sessionQuestionId": format!("{session}#{qkey}"),
            "questionSnapshot": { "id": qkey, "stem": format!("stem of {qkey}") },
            "userAnswer": "A",
            "correctAnswer": "A",
            "isCorrect": is_correct,
            "flagged": flagged,
            "subject": subject,
            "chapter": "ch1",
            "knowledgePoint": "kp1",
            "durationMs": 250,
            "submittedAt": submitted_at,
        })
    }

    fn input(v: serde_json::Value) -> AttemptInput {
        serde_json::from_value(v).unwrap()
    }

    fn counts(c: &BatchCounts) -> (u64, u64, u64) {
        (c.accepted, c.updated, c.duplicates)
    }

    #[test]
    fn init_migrates_early_study_schema_and_backfills_review_events() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE study_attempts (
               seq INTEGER PRIMARY KEY AUTOINCREMENT,
               idempotency_key TEXT NOT NULL UNIQUE, session_key TEXT NOT NULL,
               question_key TEXT NOT NULL, session_question_id TEXT NOT NULL DEFAULT '',
               question_snapshot TEXT NOT NULL DEFAULT '', user_answer TEXT NOT NULL DEFAULT '',
               correct_answer TEXT NOT NULL DEFAULT '', is_correct INTEGER,
               flagged INTEGER NOT NULL DEFAULT 0, subject TEXT, chapter TEXT,
               knowledge_point TEXT, submitted_at INTEGER NOT NULL, created_at INTEGER NOT NULL
             );
             CREATE TABLE study_consumers (
               consumer TEXT PRIMARY KEY, cursor INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL
             );
             CREATE TABLE study_banks (
               bank_key TEXT PRIMARY KEY, name TEXT NOT NULL, payload TEXT NOT NULL,
               question_count INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
             );
             INSERT INTO study_banks (bank_key,name,payload,question_count,created_at,updated_at)
             VALUES ('old-bank','Old Bank','{}',0,5,5);
             INSERT INTO study_attempts
               (idempotency_key,session_key,question_key,is_correct,flagged,submitted_at,created_at)
             VALUES ('old-wrong','s','q1',0,0,1,10),
                    ('old-flagged','s','q2',1,1,2,11),
                    ('old-correct','s','q3',1,0,3,12);
             INSERT INTO study_consumers (consumer,cursor,updated_at) VALUES ('chatgpt',999,20);"
        ).unwrap();

        init_tables(&conn).unwrap();
        let has_device: bool = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "device_id");
        assert!(has_device);
        let has_bank: bool = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "bank_key");
        assert!(has_bank);
        let has_duration: bool = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "duration_ms");
        assert!(has_duration);
        let has_original_id: bool = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "original_question_id");
        assert!(has_original_id);
        let has_user_answer_is_null: bool = conn
            .prepare("PRAGMA table_info(study_attempts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "user_answer_is_null");
        assert!(has_user_answer_is_null);
        let has_content_hash: bool = conn
            .prepare("PRAGMA table_info(study_banks)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "content_hash");
        assert!(has_content_hash);

        let old_attempt = fetch_attempt_by_idempotency(&conn, "old-wrong")
            .unwrap()
            .unwrap();
        assert_eq!(old_attempt.device_id, "");
        assert_eq!(old_attempt.bank_key, "");
        assert_eq!(old_attempt.original_question_id, "");
        assert_eq!(old_attempt.user_answer, Some(String::new()));
        assert_eq!(old_attempt.duration_ms, 0);

        let old_bank = run_bank_get(&conn, "old-bank").unwrap().unwrap();
        assert_eq!(old_bank.content_hash, "");

        let events: Vec<(i64, String)> = conn
            .prepare("SELECT attempt_seq, reason FROM study_review_events ORDER BY seq")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(events, vec![(1, "wrong".into()), (2, "flagged".into())]);
        let consumers: i64 = conn
            .query_row("SELECT COUNT(*) FROM study_consumers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(consumers, 0);
    }

    #[test]
    fn batch_dedup_flag_toggle_and_conflict() {
        let conn = test_conn();
        let batch = [
            input(attempt_json(
                "k1",
                "s1",
                "q1",
                json!(false),
                false,
                "math",
                1000,
            )),
            input(attempt_json(
                "k2",
                "s1",
                "q2",
                json!(true),
                false,
                "math",
                1010,
            )),
        ];
        let c = process_batch(&conn, &batch, 5000).unwrap();
        assert_eq!(counts(&c), (2, 0, 0));

        // identical retry -> duplicates
        let c = process_batch(&conn, &batch, 6000).unwrap();
        assert_eq!(counts(&c), (0, 0, 2));

        // flag toggle with identical identity -> in-place update, no new row
        let toggled = attempt_json("k1", "s1", "q1", json!(false), true, "math", 1000);
        let c = process_batch(&conn, &[input(toggled)], 7000).unwrap();
        assert_eq!(counts(&c), (0, 1, 0));

        // toggle back with submittedAt omitted (server-assigned) -> still an update
        let mut toggled_back = attempt_json("k1", "s1", "q1", json!(false), false, "math", 1000);
        toggled_back.as_object_mut().unwrap().remove("submittedAt");
        let c = process_batch(&conn, &[input(toggled_back)], 8000).unwrap();
        assert_eq!(counts(&c), (0, 1, 0));

        // immutable change with the same idempotency key -> ignored duplicate
        let mut conflict = attempt_json("k1", "s1", "q1", json!(false), false, "math", 1000);
        conflict["userAnswer"] = json!("B");
        let c = process_batch(&conn, &[input(conflict)], 9000).unwrap();
        assert_eq!(counts(&c), (0, 0, 1));

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM study_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
    }

    #[test]
    fn batch_preserves_null_user_answer() {
        let conn = test_conn();
        let mut attempt = attempt_json(
            "null-answer",
            "s-null",
            "q-null",
            json!(false),
            false,
            "math",
            1000,
        );
        attempt["userAnswer"] = serde_json::Value::Null;
        process_batch(&conn, &[input(attempt)], 5000).unwrap();

        let stored = fetch_attempt_by_idempotency(&conn, "null-answer")
            .unwrap()
            .unwrap();
        assert_eq!(stored.user_answer, None);

        let history = run_history(&conn, "q-null", 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].user_answer, None);
    }

    #[test]
    fn batch_allows_first_grade_for_previously_ungraded_attempt() {
        let conn = test_conn();
        let ungraded = attempt_json(
            "grade1",
            "s1",
            "q-grade",
            serde_json::Value::Null,
            false,
            "math",
            1000,
        );
        let c = process_batch(&conn, &[input(ungraded)], 5000).unwrap();
        assert_eq!(counts(&c), (1, 0, 0));

        let graded_wrong =
            attempt_json("grade1", "s1", "q-grade", json!(false), false, "math", 1000);
        let c = process_batch(&conn, &[input(graded_wrong)], 6000).unwrap();
        assert_eq!(counts(&c), (0, 1, 0));

        let stored = fetch_attempt_by_idempotency(&conn, "grade1")
            .unwrap()
            .unwrap();
        assert_eq!(stored.is_correct, Some(false));
        let page = run_feed(&conn, "grade-review", None, None, None, None, true, 100).unwrap();
        assert_eq!(page.attempts.len(), 1);
        assert_eq!(page.attempts[0].question_key, "q-grade");

        // A final grade cannot later be flipped.
        let regraded = attempt_json("grade1", "s1", "q-grade", json!(true), false, "math", 1000);
        let c = process_batch(&conn, &[input(regraded)], 7000).unwrap();
        assert_eq!(counts(&c), (0, 0, 1));
        let stored = fetch_attempt_by_idempotency(&conn, "grade1")
            .unwrap()
            .unwrap();
        assert_eq!(stored.is_correct, Some(false));
    }

    #[test]
    fn feed_filters_and_cursor_ack() {
        let conn = test_conn();
        let batch = [
            input(attempt_json(
                "a1",
                "s1",
                "q1",
                json!(false),
                false,
                "math",
                1000,
            )),
            input(attempt_json(
                "a2",
                "s1",
                "q2",
                json!(true),
                false,
                "math",
                1010,
            )),
            input(attempt_json(
                "a3",
                "s1",
                "q3",
                json!(true),
                true,
                "math",
                1020,
            )),
            input(attempt_json(
                "a4",
                "s1",
                "q4",
                serde_json::Value::Null,
                false,
                "math",
                1030,
            )),
            input(attempt_json(
                "a5",
                "s1",
                "q5",
                json!(false),
                true,
                "physics",
                1040,
            )),
        ];
        process_batch(&conn, &batch, 5000).unwrap();

        // only wrong or flagged attempts, seq ASC
        let page = run_feed(&conn, "cli", None, None, None, None, true, 100).unwrap();
        assert_eq!(page.stored_cursor, 0);
        let seqs: Vec<i64> = page.attempts.iter().map(|a| a.seq).collect();
        assert_eq!(seqs, vec![1, 3, 5]);
        let feed_seqs: Vec<i64> = page.attempts.iter().filter_map(|a| a.feed_seq).collect();
        assert_eq!(feed_seqs, vec![1, 2, 3]);
        assert_eq!(page.next_cursor, 3);
        assert_eq!(page.attempts[0].wrong_count, Some(1));
        assert_eq!(page.attempts[0].attempt_count, Some(1));

        // exclude flagged-only attempts without changing the event cursor model
        let wrong_only = run_feed(&conn, "wrong-only", None, None, None, None, false, 100).unwrap();
        let wrong_only_seqs: Vec<i64> = wrong_only.attempts.iter().map(|a| a.seq).collect();
        assert_eq!(wrong_only_seqs, vec![1, 5]);

        // feed must not implicitly advance the cursor
        let stored: Option<i64> = conn
            .query_row(
                "SELECT cursor FROM study_consumers WHERE consumer = 'cli'",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(stored, None);

        // explicit after overrides the stored cursor
        let page = run_feed(&conn, "cli", Some(2), None, None, None, true, 100).unwrap();
        assert_eq!(page.stored_cursor, 2);
        let seqs: Vec<i64> = page.attempts.iter().map(|a| a.seq).collect();
        assert_eq!(seqs, vec![5]);

        // subject filter
        let page = run_feed(&conn, "cli", None, Some("math"), None, None, true, 100).unwrap();
        let seqs: Vec<i64> = page.attempts.iter().map(|a| a.seq).collect();
        assert_eq!(seqs, vec![1, 3]);
        let page = run_feed(&conn, "cli-case", None, Some("MATH"), None, None, true, 100).unwrap();
        let seqs: Vec<i64> = page.attempts.iter().map(|a| a.seq).collect();
        assert_eq!(seqs, vec![1, 3]);

        // ack stores the cursor monotonically; ahead-of-max is rejected
        assert_eq!(run_ack(&conn, "cli", 3).unwrap(), 3);
        assert_eq!(run_ack(&conn, "cli", 2).unwrap(), 3);
        let ahead = run_ack(&conn, "cli", 4).unwrap_err();
        assert_eq!(ahead.0, StatusCode::BAD_REQUEST);

        // after ack the feed is empty and nextCursor stays at the stored cursor
        let page = run_feed(&conn, "cli", None, None, None, None, true, 100).unwrap();
        assert!(page.attempts.is_empty());
        assert_eq!(page.stored_cursor, 3);
        assert_eq!(page.next_cursor, 3);

        // a separate consumer starts from 0
        let page = run_feed(&conn, "phone", None, None, None, None, true, 100).unwrap();
        assert_eq!(page.stored_cursor, 0);
    }

    #[test]
    fn feed_bank_filter_and_monotonic_high_water() {
        let conn = test_conn();
        let first = attempt_json("b1", "s1", "q1", json!(false), false, "math", 1000);
        let mut second = attempt_json("b2", "s1", "q2", json!(false), false, "math", 1010);
        second["bankKey"] = json!("bank-other");
        process_batch(&conn, &[input(first), input(second)], 5000).unwrap();

        let page = run_feed(
            &conn,
            "bank-filter",
            None,
            None,
            None,
            Some("bank-other"),
            true,
            100,
        )
        .unwrap();
        assert_eq!(page.attempts.len(), 1);
        assert_eq!(page.attempts[0].bank_key, "bank-other");

        // Simulate future compaction: sqlite_sequence must still preserve the
        // event high-water so cursor validation never moves backwards.
        conn.execute("DELETE FROM study_review_events WHERE seq = 2", [])
            .unwrap();
        assert_eq!(run_ack(&conn, "bank-filter", 2).unwrap(), 2);
        let ahead = run_ack(&conn, "bank-filter", 3).unwrap_err();
        assert_eq!(ahead.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn late_flag_after_ack_creates_new_review_event() {
        let conn = test_conn();
        let initial = [
            input(attempt_json(
                "late1",
                "s1",
                "q1",
                json!(true),
                false,
                "math",
                1000,
            )),
            input(attempt_json(
                "late2",
                "s1",
                "q2",
                json!(false),
                false,
                "math",
                1010,
            )),
        ];
        process_batch(&conn, &initial, 5000).unwrap();

        let first = run_feed(&conn, "chatgpt", None, None, None, None, true, 100).unwrap();
        assert_eq!(first.attempts.len(), 1);
        assert_eq!(first.attempts[0].question_key, "q2");
        assert_eq!(first.next_cursor, 1);
        assert_eq!(run_ack(&conn, "chatgpt", 1).unwrap(), 1);

        let flagged = attempt_json("late1", "s1", "q1", json!(true), true, "math", 1000);
        let update_counts = process_batch(&conn, &[input(flagged)], 6000).unwrap();
        assert_eq!(counts(&update_counts), (0, 1, 0));

        let second = run_feed(&conn, "chatgpt", None, None, None, None, true, 100).unwrap();
        assert_eq!(second.attempts.len(), 1);
        assert_eq!(second.attempts[0].question_key, "q1");
        assert!(second.attempts[0].flagged);
        assert_eq!(second.attempts[0].feed_seq, Some(2));
        assert_eq!(second.next_cursor, 2);
    }

    #[test]
    fn direct_flag_update_is_idempotent_and_creates_review_event_on_rising_edge() {
        let conn = test_conn();
        let initial = attempt_json("flag-direct", "s1", "q-flag", json!(true), false, "math", 1000);
        process_batch(&conn, &[input(initial)], 5000).unwrap();

        let updated = run_flag_update(&conn, "flag-direct", true, 6000).unwrap().unwrap();
        assert!(updated.flagged);
        let page = run_feed(&conn, "flag-direct-consumer", None, None, None, None, true, 100).unwrap();
        assert_eq!(page.attempts.len(), 1);
        assert_eq!(page.attempts[0].question_key, "q-flag");
        let first_cursor = page.next_cursor;

        // Repeating the same state is idempotent and does not create another event.
        let updated = run_flag_update(&conn, "flag-direct", true, 7000).unwrap().unwrap();
        assert!(updated.flagged);
        let page = run_feed(&conn, "flag-direct-consumer", Some(first_cursor), None, None, None, true, 100).unwrap();
        assert!(page.attempts.is_empty());

        // Unflagging removes it from active attention; re-flagging emits a new event.
        let updated = run_flag_update(&conn, "flag-direct", false, 8000).unwrap().unwrap();
        assert!(!updated.flagged);
        let updated = run_flag_update(&conn, "flag-direct", true, 9000).unwrap().unwrap();
        assert!(updated.flagged);
        let page = run_feed(&conn, "flag-direct-consumer", Some(first_cursor), None, None, None, true, 100).unwrap();
        assert_eq!(page.attempts.len(), 1);
        assert!(page.next_cursor > first_cursor);

        assert!(run_flag_update(&conn, "missing", true, 10000).unwrap().is_none());
    }

    #[test]
    fn history_newest_first() {
        let conn = test_conn();
        let batch = [
            input(attempt_json(
                "h1",
                "s1",
                "q9",
                json!(false),
                false,
                "math",
                1000,
            )),
            input(attempt_json(
                "h2",
                "s2",
                "q9",
                json!(true),
                false,
                "math",
                2000,
            )),
            input(attempt_json(
                "h3",
                "s3",
                "q9",
                json!(false),
                true,
                "math",
                3000,
            )),
        ];
        process_batch(&conn, &batch, 5000).unwrap();

        let all = run_history(&conn, "q9", 100).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].idempotency_key, "h3");
        assert_eq!(all[1].idempotency_key, "h2");
        assert_eq!(all[2].idempotency_key, "h1");
        assert!(all[0].flagged);
        assert_eq!(all[0].device_id, "test-device");
        assert_eq!(all[0].question_snapshot["id"], "q9");

        let limited = run_history(&conn, "q9", 2).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].idempotency_key, "h3");

        assert!(run_history(&conn, "missing", 100).unwrap().is_empty());
    }

    #[test]
    fn sessions_inferred_finish_and_latest() {
        let conn = test_conn();
        let s1 = [
            input(attempt_json(
                "x1",
                "s1",
                "q1",
                json!(true),
                false,
                "math",
                100,
            )),
            input(attempt_json(
                "x2",
                "s1",
                "q2",
                json!(false),
                true,
                "math",
                110,
            )),
        ];
        process_batch(&conn, &s1, 5000).unwrap();
        let s2 = [
            input(attempt_json(
                "y1",
                "s2",
                "q3",
                json!(true),
                false,
                "math",
                200,
            )),
            input(attempt_json(
                "y2",
                "s2",
                "q4",
                json!(false),
                false,
                "math",
                210,
            )),
        ];
        process_batch(&conn, &s2, 6000).unwrap();

        // latest = most recently started session, with attempt aggregates
        let latest = run_latest(&conn).unwrap().unwrap();
        assert_eq!(latest.session_key, "s2");
        assert_eq!(latest.started_at, Some(200));
        assert_eq!(latest.finished_at, None);
        assert_eq!(
            (latest.total, latest.correct, latest.wrong, latest.flagged),
            (2, 1, 1, 0)
        );

        // finishing an older session does not make it "latest"
        upsert_session(&conn, "s1", None, Some(300), 7000).unwrap();
        let (started, finished) = fetch_session_row(&conn, "s1").unwrap();
        assert_eq!((started, finished), (Some(100), Some(300)));
        let latest = run_latest(&conn).unwrap().unwrap();
        assert_eq!(latest.session_key, "s2");

        upsert_session(&conn, "s2", None, Some(250), 8000).unwrap();
        let latest = run_latest(&conn).unwrap().unwrap();
        assert_eq!(latest.session_key, "s2");
        assert_eq!(latest.finished_at, Some(250));
        assert_eq!(latest.total, 2);

        // finishing an unknown session creates it without started_at
        upsert_session(&conn, "s3", None, Some(999), 9000).unwrap();
        let (started, finished) = fetch_session_row(&conn, "s3").unwrap();
        assert_eq!((started, finished), (None, Some(999)));
    }

    fn valid_bank() -> serde_json::Value {
        json!({
            "schemaVersion": 1,
            "key": "sgcc-safety",
            "name": "Bank One",
            "version": 1,
            "metadata": { "exam": "国家电网计算机类", "outlineVersion": "2026", "subject": "信息新技术" },
            "subject": "电气安全",
            "chapter": "第一章",
            "tags": ["sgcc", "safety"],
            "sourceMeta": { "origin": "test" },
            "questions": [
                { "id": "qid-1", "type": "single_choice", "stem": "Pick one", "options": ["a", "b", "c", "d"], "answer": "A", "analysis": "A is first" },
                { "stableKey": "sk-multi", "type": "multi_choice", "stem": "Pick two", "options": ["a", "b", "c"], "answer": "AB", "analysis": "A and B", "difficulty": "hard" },
                { "stableKey": "sk-true-false", "type": "true_false", "stem": "Sky is blue", "answer": "true", "analysis": "Usually" },
                { "stableKey": "sk-fill-blank", "type": "fill_blank", "stem": "One plus one is", "answer": "2", "analysis": "Arithmetic" }
            ]
        })
    }

    fn bank_with_questions(questions: serde_json::Value) -> serde_json::Value {
        let mut bank = valid_bank();
        bank["questions"] = questions;
        bank
    }

    fn question_of(question_type: &str) -> serde_json::Value {
        let mut question = json!({
            "stableKey": format!("sk-{question_type}"),
            "type": question_type,
            "stem": "stem",
            "analysis": "analysis",
        });
        match question_type {
            "single_choice" => {
                question["options"] = json!(["a", "b", "c", "d"]);
                question["answer"] = json!("A");
            }
            "multi_choice" => {
                question["options"] = json!(["a", "b", "c", "d"]);
                question["answer"] = json!("AB");
            }
            "true_false" => question["answer"] = json!("true"),
            "fill_blank" => question["answer"] = json!("filled"),
            _ => {}
        }
        question
    }

    fn question_err(question: serde_json::Value) -> String {
        validate_bank(&bank_with_questions(json!([question]))).unwrap_err()
    }

    #[test]
    fn bank_accepts_direct_cli_payload() {
        // tools/exameowctl `bank import` posts the validated bank JSON itself
        // as the request body: no bankKey envelope, key comes from `key`.
        let (key, name, payload, count) = validate_bank(&valid_bank()).unwrap();
        assert_eq!(key, "sgcc-safety");
        assert_eq!(name, "Bank One");
        assert_eq!(count, 4);
        assert_eq!(payload["schemaVersion"], json!(1));
        assert_eq!(payload["key"], json!("sgcc-safety"));

        // empty question list is legal
        let (_, _, _, count) = validate_bank(&bank_with_questions(json!([]))).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn bank_rejects_legacy_flat_payload() {
        // The pre-schema contract posted a flat {bankKey, name, questions}
        // body; only schemaVersion 1 banks are accepted now.
        let flat = json!({ "bankKey": "b1", "name": "N", "questions": [] });
        let err = validate_bank(&flat).unwrap_err();
        assert!(err.contains("unknown field"), "{err}");

        let mut flat = flat;
        flat["schemaVersion"] = json!(1);
        let err = validate_bank(&flat).unwrap_err();
        assert!(err.contains("unknown field"), "{err}");
    }

    #[test]
    fn bank_rejects_short_answer_type() {
        let err = question_err(json!({
            "stableKey": "sk-1", "type": "short_answer",
            "stem": "s", "answer": "essay", "analysis": "a"
        }));
        assert!(err.contains("short_answer"), "{err}");
    }

    #[test]
    fn bank_rejects_duplicate_stable_keys() {
        let bank = bank_with_questions(json!([
            { "stableKey": "dup", "type": "fill_blank", "stem": "s", "answer": "a", "analysis": "n" },
            { "stableKey": "dup", "type": "fill_blank", "stem": "s2", "answer": "b", "analysis": "n" }
        ]));
        let err = validate_bank(&bank).unwrap_err();
        assert!(err.contains("duplicate stableKey"), "{err}");
    }

    #[test]
    fn bank_rejects_duplicate_ids() {
        let bank = bank_with_questions(json!([
            { "id": "dup-id", "stableKey": "key-1", "type": "fill_blank", "stem": "s", "answer": "a", "analysis": "n" },
            { "id": "dup-id", "stableKey": "key-2", "type": "fill_blank", "stem": "s2", "answer": "b", "analysis": "n" }
        ]));
        let err = validate_bank(&bank).unwrap_err();
        assert!(err.contains("duplicate id"), "{err}");
    }

    #[test]
    fn bank_rejects_bad_answers() {
        let mut q = question_of("single_choice");
        q["answer"] = json!("a");
        assert!(question_err(q).contains("uppercase"));

        let mut q = question_of("single_choice");
        q["answer"] = json!("AB");
        assert!(question_err(q).contains("exactly one option letter"));

        let mut q = question_of("single_choice");
        q["answer"] = json!("E"); // options are A-D
        assert!(question_err(q).contains("do not exist"));

        let mut q = question_of("multi_choice");
        q["answer"] = json!("AA");
        assert!(question_err(q).contains("repeat"));

        let mut q = question_of("true_false");
        q["answer"] = json!("True");
        let err = question_err(q);
        assert!(err.contains("true_false answer must be exactly"), "{err}");

        let mut q = question_of("fill_blank");
        q["answer"] = json!("  ");
        assert!(question_err(q).contains("whitespace-only"));

        let mut q = question_of("fill_blank");
        q.as_object_mut().unwrap().remove("answer");
        assert!(question_err(q).contains("missing required field: answer"));
    }

    #[test]
    fn bank_rejects_bad_options() {
        let mut q = question_of("single_choice");
        q["options"] = json!(["1", "2", "3", "4", "5", "6"]);
        assert!(question_err(q).contains("2-5 options"));

        let mut q = question_of("single_choice");
        q["options"] = json!(["1"]);
        assert!(question_err(q).contains("2-5 options"));

        let mut q = question_of("single_choice");
        q["options"] = json!(["a", " "]);
        assert!(question_err(q).contains("whitespace-only"));

        let mut q = question_of("true_false");
        q["options"] = json!(["true", "false"]);
        assert!(question_err(q).contains("only allowed for single_choice/multi_choice"));

        let mut q = question_of("multi_choice");
        q.as_object_mut().unwrap().remove("options");
        assert!(question_err(q).contains("missing required field: options"));
    }

    #[test]
    fn bank_question_required_and_optional_fields() {
        let mut q = question_of("fill_blank");
        q.as_object_mut().unwrap().remove("stem");
        assert!(question_err(q).contains("missing required field: stem"));

        let mut q = question_of("fill_blank");
        q.as_object_mut().unwrap().remove("analysis");
        assert!(question_err(q).contains("missing required field: analysis"));

        let mut q = question_of("fill_blank");
        q["stem"] = json!("  ");
        assert!(question_err(q).contains("whitespace-only"));

        let mut q = question_of("fill_blank");
        q.as_object_mut().unwrap().remove("stableKey");
        assert!(question_err(q).contains("'id' or 'stableKey'"));

        let mut q = question_of("fill_blank");
        q["id"] = json!("  ");
        assert!(question_err(q).contains("must not be empty"));

        let mut q = question_of("fill_blank");
        q["oops"] = json!(true);
        assert!(question_err(q).contains("unknown field"));

        let mut q = question_of("fill_blank");
        q["difficulty"] = json!("extreme");
        assert!(question_err(q).contains("difficulty must be one of"));

        let mut q = question_of("fill_blank");
        q["tags"] = json!(["a", ""]);
        assert!(question_err(q).contains("whitespace-only"));

        let mut q = question_of("fill_blank");
        q["sourceMeta"] = json!("x");
        assert!(question_err(q).contains("sourceMeta must be an object"));

        let mut q = question_of("fill_blank");
        q["difficulty"] = json!("medium");
        q["knowledgePoint"] = json!("kp");
        q["subject"] = json!("sub");
        q["tags"] = json!(["t"]);
        assert!(validate_bank(&bank_with_questions(json!([q]))).is_ok());
    }

    #[test]
    fn bank_content_version_and_metadata_rules() {
        let mut bank = valid_bank();
        bank.as_object_mut().unwrap().remove("version");
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("missing required field: version"));

        let mut bank = valid_bank();
        bank["version"] = json!(0);
        assert!(validate_bank(&bank).unwrap_err().contains("integer >= 1"));

        let mut bank = valid_bank();
        bank.as_object_mut().unwrap().remove("metadata");
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("missing required field: metadata"));

        let mut bank = valid_bank();
        bank["metadata"] = json!({ "exam": "国家电网计算机类", "outlineVersion": "2026" });
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("metadata field: subject"));

        let mut bank = valid_bank();
        bank["key"] = json!("国网-key");
        assert!(validate_bank(&bank).unwrap_err().contains("ASCII letters"));
    }

    #[test]
    fn bank_schema_version_and_key_rules() {
        let mut bank = valid_bank();
        bank.as_object_mut().unwrap().remove("schemaVersion");
        let err = validate_bank(&bank).unwrap_err();
        assert!(
            err.contains("missing required field: schemaVersion"),
            "{err}"
        );

        for bad_version in [json!(2), json!("1"), json!(true), json!(1.5)] {
            let mut bank = valid_bank();
            bank["schemaVersion"] = bad_version.clone();
            assert!(
                validate_bank(&bank).is_err(),
                "schemaVersion {bad_version} must be rejected"
            );
        }

        let mut bank = valid_bank();
        bank.as_object_mut().unwrap().remove("key");
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("missing required field: key"));

        let bad_keys = [
            String::new(),
            "my bank".to_string(),
            "a/b".to_string(),
            "a\\b".to_string(),
            format!("a\u{7f}b"),
            "x".repeat(129),
        ];
        for bad_key in &bad_keys {
            let mut bank = valid_bank();
            bank["key"] = json!(bad_key);
            assert!(
                validate_bank(&bank).is_err(),
                "key {bad_key:?} must be rejected"
            );
        }

        let mut bank = valid_bank();
        bank["key"] = json!("x".repeat(128));
        assert!(validate_bank(&bank).is_ok());

        // stableKey follows the same identifier rules
        let mut bank = bank_with_questions(json!([question_of("fill_blank")]));
        bank["questions"][0]["stableKey"] = json!("bad key");
        assert!(validate_bank(&bank).unwrap_err().contains("whitespace"));

        let mut bank = valid_bank();
        bank.as_object_mut().unwrap().remove("name");
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("missing required field: name"));
        let mut bank = valid_bank();
        bank["name"] = json!("   ");
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("whitespace-only"));

        let mut bank = valid_bank();
        bank.as_object_mut().unwrap().remove("questions");
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("missing required field: questions"));
        let mut bank = valid_bank();
        bank["questions"] = json!({});
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("questions must be a list"));
    }

    #[test]
    fn bank_unknown_and_optional_bank_fields() {
        let mut bank = valid_bank();
        bank["bankKey"] = json!("sgcc-safety");
        let err = validate_bank(&bank).unwrap_err();
        assert!(err.contains("unknown field"), "{err}");

        let mut bank = valid_bank();
        bank["subject"] = json!("  ");
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("whitespace-only"));

        let mut bank = valid_bank();
        bank["tags"] = json!("not-a-list");
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("tags must be a list"));

        let mut bank = valid_bank();
        bank["tags"] = json!(["ok", ""]);
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("whitespace-only"));

        let mut bank = valid_bank();
        bank["tags"] = json!(["dup", "dup"]);
        assert!(validate_bank(&bank).unwrap_err().contains("duplicate tag"));

        let mut bank = valid_bank();
        bank["sourceMeta"] = json!([1]);
        assert!(validate_bank(&bank)
            .unwrap_err()
            .contains("sourceMeta must be an object"));
    }

    #[test]
    fn bank_wrapper_contract() {
        // matching wrapper accepted; the key comes from the nested bank
        let wrapped = json!({ "bankKey": "sgcc-safety", "bank": valid_bank() });
        let (key, name, _, count) = validate_bank(&wrapped).unwrap();
        assert_eq!(key, "sgcc-safety");
        assert_eq!(name, "Bank One");
        assert_eq!(count, 4);

        // wrapper bankKey may be omitted
        let (key, _, _, _) = validate_bank(&json!({ "bank": valid_bank() })).unwrap();
        assert_eq!(key, "sgcc-safety");

        // wrapper bankKey must match the nested key
        let mismatched = json!({ "bankKey": "other", "bank": valid_bank() });
        let err = validate_bank(&mismatched).unwrap_err();
        assert!(err.contains("does not match"), "{err}");

        // non-string wrapper bankKey is rejected
        let bad_type = json!({ "bankKey": 123, "bank": valid_bank() });
        let err = validate_bank(&bad_type).unwrap_err();
        assert!(err.contains("bankKey must be a string"), "{err}");

        // the nested bank must itself be schemaVersion 1 compliant
        let legacy = json!({ "bankKey": "b2", "bank": { "name": "Nested", "questions": [] } });
        let err = validate_bank(&legacy).unwrap_err();
        assert!(
            err.contains("missing required field: schemaVersion"),
            "{err}"
        );

        // bankKey cannot substitute for a missing nested key
        let mut keyless = valid_bank();
        keyless.as_object_mut().unwrap().remove("key");
        let err = validate_bank(&json!({ "bankKey": "sgcc-safety", "bank": keyless })).unwrap_err();
        assert!(err.contains("missing required field: key"), "{err}");

        // unknown wrapper fields are rejected
        let extra = json!({ "bank": valid_bank(), "oops": true });
        let err = validate_bank(&extra).unwrap_err();
        assert!(err.contains("unknown field"), "{err}");
    }

    #[test]
    fn bank_rejects_too_many_questions() {
        let questions: Vec<serde_json::Value> = (0..=MAX_BANK_QUESTIONS)
            .map(|i| {
                json!({
                    "stableKey": format!("sk-{i}"),
                    "type": "fill_blank",
                    "stem": "stem",
                    "answer": "answer",
                    "analysis": "analysis",
                })
            })
            .collect();
        let bank = bank_with_questions(json!(questions));
        let err = validate_bank(&bank).unwrap_err();
        assert!(err.contains("exceed"), "{err}");
    }

    #[test]
    fn bank_rejects_oversized_payload() {
        let bank = bank_with_questions(json!([{
            "stableKey": "sk-big",
            "type": "fill_blank",
            "stem": "x".repeat(MAX_BANK_PAYLOAD_BYTES),
            "answer": "a",
            "analysis": "n",
        }]));
        let err = validate_bank(&bank).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
    }

    #[test]
    fn bank_storage_roundtrip() {
        let conn = test_conn();
        let bank = valid_bank();
        let (key, name, payload, count) = validate_bank(&bank).unwrap();
        let payload_json = serde_json::to_string(&payload).unwrap();
        run_bank_import(&conn, &key, &name, &payload_json, count, 1111).unwrap();
        let stored = run_bank_get(&conn, &key).unwrap().unwrap();
        assert_eq!(stored.name, "Bank One");
        assert_eq!(stored.question_count, 4);
        assert_eq!(stored.payload, payload_json);
        assert_eq!(stored.content_hash, sha256_hex(&payload_json));
        assert_eq!(stored.created_at, 1111);
        assert_eq!(stored.updated_at, 1111);

        // Identical content is idempotent and does not force mobile/web clients
        // to re-download the same bank.
        run_bank_import(&conn, &key, &name, &payload_json, count, 2222).unwrap();
        let stored = run_bank_get(&conn, &key).unwrap().unwrap();
        assert_eq!(stored.created_at, 1111);
        assert_eq!(stored.updated_at, 1111);

        let mut changed = bank.clone();
        changed["name"] = json!("Bank One v2");
        let (key2, name2, payload2, count2) = validate_bank(&changed).unwrap();
        let payload_json2 = serde_json::to_string(&payload2).unwrap();
        run_bank_import(&conn, &key2, &name2, &payload_json2, count2, 3333).unwrap();
        let stored = run_bank_get(&conn, &key).unwrap().unwrap();
        assert_eq!(stored.name, "Bank One v2");
        assert_eq!(stored.created_at, 1111);
        assert_eq!(stored.updated_at, 3333);
        assert_ne!(stored.content_hash, sha256_hex(&payload_json));

        assert!(run_bank_get(&conn, "missing").unwrap().is_none());
    }

    #[test]
    fn bearer_token_matching() {
        assert!(bearer_matches("Bearer tok123", "tok123"));
        assert!(!bearer_matches("Bearer wrong", "tok123"));
        assert!(!bearer_matches("bearer tok123", "tok123"));
        assert!(!bearer_matches("tok123", "tok123"));
        assert!(!bearer_matches("", "tok123"));
    }
}
