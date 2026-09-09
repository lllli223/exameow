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
const MAX_KEY_LEN: usize = 256;
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
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS study_attempts (
          seq INTEGER PRIMARY KEY AUTOINCREMENT,
          idempotency_key TEXT NOT NULL UNIQUE,
          session_key TEXT NOT NULL,
          question_key TEXT NOT NULL,
          session_question_id TEXT NOT NULL DEFAULT '',
          question_snapshot TEXT NOT NULL DEFAULT '',
          user_answer TEXT NOT NULL DEFAULT '',
          correct_answer TEXT NOT NULL DEFAULT '',
          is_correct INTEGER,
          flagged INTEGER NOT NULL DEFAULT 0,
          subject TEXT,
          chapter TEXT,
          knowledge_point TEXT,
          submitted_at INTEGER NOT NULL,
          created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_study_attempts_session ON study_attempts(session_key);
        CREATE INDEX IF NOT EXISTS idx_study_attempts_question ON study_attempts(question_key);
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
          question_count INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| e.to_string())
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
        .route("/sessions/finish", post(sessions_finish_handler))
        .route("/sessions/latest", get(sessions_latest_handler))
        .route("/feed", get(feed_handler))
        .route("/feed/ack", post(feed_ack_handler))
        .route(
            "/questions/{question_key}/history",
            get(question_history_handler),
        )
        .route("/banks/import", post(banks_import_handler))
        .route("/banks", get(banks_list_handler))
        .route("/banks/{bank_key}", get(banks_get_handler))
        .route("/health", get(health_handler))
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
    #[serde(rename = "questionKey")]
    pub question_key: Option<String>,
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
    #[serde(rename = "submittedAt")]
    pub submitted_at: Option<i64>,
}

#[derive(Debug)]
pub struct AttemptRow {
    pub idempotency_key: String,
    pub session_key: String,
    pub question_key: String,
    pub session_question_id: String,
    pub question_snapshot: String,
    pub user_answer: String,
    pub correct_answer: String,
    pub is_correct: Option<bool>,
    pub flagged: bool,
    pub subject: Option<String>,
    pub chapter: Option<String>,
    pub knowledge_point: Option<String>,
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
    #[serde(rename = "questionKey")]
    pub question_key: String,
    #[serde(rename = "sessionQuestionId")]
    pub session_question_id: String,
    #[serde(rename = "questionSnapshot")]
    pub question_snapshot: serde_json::Value,
    #[serde(rename = "userAnswer")]
    pub user_answer: String,
    #[serde(rename = "correctAnswer")]
    pub correct_answer: String,
    #[serde(rename = "isCorrect")]
    pub is_correct: Option<bool>,
    pub flagged: bool,
    pub subject: Option<String>,
    pub chapter: Option<String>,
    #[serde(rename = "knowledgePoint")]
    pub knowledge_point: Option<String>,
    #[serde(rename = "submittedAt")]
    pub submitted_at: i64,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(skip)]
    pub snapshot_raw: String,
}

const ATTEMPT_COLUMNS: &str = "seq, idempotency_key, session_key, question_key, session_question_id, question_snapshot, user_answer, correct_answer, is_correct, flagged, subject, chapter, knowledge_point, submitted_at, created_at";

fn map_attempt(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttemptOut> {
    let snapshot_raw: String = row.get(5)?;
    let question_snapshot = if snapshot_raw.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&snapshot_raw).unwrap_or(serde_json::Value::Null)
    };
    Ok(AttemptOut {
        seq: row.get(0)?,
        idempotency_key: row.get(1)?,
        session_key: row.get(2)?,
        question_key: row.get(3)?,
        session_question_id: row.get(4)?,
        question_snapshot,
        user_answer: row.get(6)?,
        correct_answer: row.get(7)?,
        is_correct: row.get::<_, Option<i64>>(8)?.map(|v| v != 0),
        flagged: row.get::<_, i64>(9)? != 0,
        subject: row.get(10)?,
        chapter: row.get(11)?,
        knowledge_point: row.get(12)?,
        submitted_at: row.get(13)?,
        created_at: row.get(14)?,
        snapshot_raw,
    })
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
    let question_key = require_key(input.question_key.clone(), "questionKey")?;
    let session_question_id = optional_text(
        input.session_question_id.clone(),
        "sessionQuestionId",
        MAX_KEY_LEN,
    )?;
    let question_snapshot = normalize_snapshot(input.question_snapshot.as_ref())?;
    let user_answer = optional_text(input.user_answer.clone(), "userAnswer", MAX_ANSWER_LEN)?;
    let correct_answer = optional_text(
        input.correct_answer.clone(),
        "correctAnswer",
        MAX_ANSWER_LEN,
    )?;
    let subject = optional_tag(input.subject.clone(), "subject")?;
    let chapter = optional_tag(input.chapter.clone(), "chapter")?;
    let knowledge_point = optional_tag(input.knowledge_point.clone(), "knowledgePoint")?;
    let (submitted_at, submitted_at_explicit) = match input.submitted_at {
        Some(v) if v < 0 => return Err("submittedAt must not be negative".to_string()),
        Some(v) => (v, true),
        None => (now, false),
    };
    Ok(AttemptRow {
        idempotency_key,
        session_key,
        question_key,
        session_question_id,
        question_snapshot,
        user_answer,
        correct_answer,
        is_correct: input.is_correct,
        flagged: input.flagged.unwrap_or(false),
        subject,
        chapter,
        knowledge_point,
        submitted_at,
        submitted_at_explicit,
    })
}

/// All fields except `flagged` must be identical for a retry with the same
/// idempotency key. `submittedAt` may be omitted by the client (server
/// assigns it), so an omitted value matches whatever is stored.
fn rows_match(existing: &AttemptOut, incoming: &AttemptRow) -> bool {
    if existing.session_key != incoming.session_key
        || existing.question_key != incoming.question_key
        || existing.session_question_id != incoming.session_question_id
        || existing.snapshot_raw != incoming.question_snapshot
        || existing.user_answer != incoming.user_answer
        || existing.correct_answer != incoming.correct_answer
        || existing.is_correct != incoming.is_correct
        || existing.subject != incoming.subject
        || existing.chapter != incoming.chapter
        || existing.knowledge_point != incoming.knowledge_point
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
                   (idempotency_key, session_key, question_key, session_question_id,
                    question_snapshot, user_answer, correct_answer, is_correct, flagged,
                    subject, chapter, knowledge_point, submitted_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(idempotency_key) DO NOTHING",
                params![
                    row.idempotency_key,
                    row.session_key,
                    row.question_key,
                    row.session_question_id,
                    row.question_snapshot,
                    row.user_answer,
                    row.correct_answer,
                    row.is_correct,
                    row.flagged,
                    row.subject,
                    row.chapter,
                    row.knowledge_point,
                    row.submitted_at,
                    now,
                ],
            )
            .map_err(|e| e.to_string())?;
        if inserted > 0 {
            counts.accepted += 1;
            touched_sessions.push(row.session_key.clone());
            continue;
        }
        // Duplicate idempotency key: identical payload -> duplicate; only
        // `flagged` changed -> update in place (no new history row); any other
        // change -> ignored (immutable fields cannot be rewritten).
        let existing = fetch_attempt_by_idempotency(&tx, &row.idempotency_key)?
            .ok_or_else(|| "inconsistent duplicate idempotency key".to_string())?;
        if !rows_match(&existing, row) {
            counts.duplicates += 1;
            continue;
        }
        if existing.flagged != row.flagged {
            tx.execute(
                "UPDATE study_attempts SET flagged = ?1 WHERE seq = ?2",
                params![row.flagged, existing.seq],
            )
            .map_err(|e| e.to_string())?;
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
        "SELECT {ATTEMPT_COLUMNS} FROM study_attempts
         WHERE seq > ?1
           AND (is_correct = 0 OR flagged = 1)
           AND (?2 IS NULL OR subject = ?2)
           AND (?3 IS NULL OR chapter = ?3)
         ORDER BY seq ASC LIMIT ?4"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    let attempts: Vec<AttemptOut> = stmt
        .query_map(params![start, subject, chapter, limit], map_attempt)
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?
        .filter_map(|r| r.ok())
        .collect();
    let next_cursor = attempts.last().map(|a| a.seq).unwrap_or(start);
    Ok(FeedPage {
        stored_cursor: start,
        next_cursor,
        attempts,
    })
}

fn run_ack(conn: &Connection, consumer: &str, cursor: i64) -> Result<i64, Err> {
    let max_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM study_attempts",
            [],
            |r| r.get(0),
        )
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    if cursor > max_seq {
        return Err(err_msg(
            StatusCode::BAD_REQUEST,
            "cursor_ahead",
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

/// Validate an import payload and return (bankKey, name, payload, questionCount).
/// The payload is either the `bank` object (when present) or the whole body.
fn validate_bank(
    body: &serde_json::Value,
) -> Result<(String, String, serde_json::Value, usize), String> {
    let obj = body
        .as_object()
        .ok_or_else(|| "body must be a JSON object".to_string())?;
    let bank_key = obj
        .get("bankKey")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if bank_key.is_empty() {
        return Err("bankKey is required".to_string());
    }
    if bank_key.len() > MAX_KEY_LEN {
        return Err(format!("bankKey exceeds {MAX_KEY_LEN} characters"));
    }
    let payload = match obj.get("bank") {
        None => body.clone(),
        Some(v) if v.is_object() => v.clone(),
        Some(_) => return Err("bank must be a JSON object".to_string()),
    };
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if name.is_empty() {
        return Err("bank name is required".to_string());
    }
    if name.len() > MAX_BANK_NAME_LEN {
        return Err(format!("bank name exceeds {MAX_BANK_NAME_LEN} characters"));
    }
    let questions = payload
        .get("questions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "bank questions must be a JSON array".to_string())?;
    if questions.len() > MAX_BANK_QUESTIONS {
        return Err(format!("questions exceed {MAX_BANK_QUESTIONS} items"));
    }
    for (i, q) in questions.iter().enumerate() {
        if !q.is_object() {
            return Err(format!("questions[{i}] must be a JSON object"));
        }
        let options = q.get("options").and_then(|v| v.as_array());
        if options.is_some_and(|o| o.len() > 5) {
            let qtype = q.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if !matches!(qtype, "true_false" | "fill_blank" | "short_answer") {
                return Err(format!(
                    "questions[{i}] is a choice question with more than 5 options"
                ));
            }
        }
    }
    let serialized = serde_json::to_string(&payload)
        .map_err(|e| format!("bank payload is not serializable: {e}"))?;
    if serialized.len() > MAX_BANK_PAYLOAD_BYTES {
        return Err(format!(
            "bank payload exceeds {MAX_BANK_PAYLOAD_BYTES} bytes"
        ));
    }
    let question_count = questions.len();
    Ok((
        bank_key.to_string(),
        name.to_string(),
        payload,
        question_count,
    ))
}

struct StoredBank {
    name: String,
    payload: String,
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
    conn.execute(
        "INSERT INTO study_banks (bank_key, name, payload, question_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(bank_key) DO UPDATE SET
           name = excluded.name,
           payload = excluded.payload,
           question_count = excluded.question_count,
           updated_at = excluded.updated_at",
        params![bank_key, name, payload_json, question_count as i64, now],
    )
    .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    Ok(())
}

fn run_bank_get(conn: &Connection, bank_key: &str) -> Result<Option<StoredBank>, Err> {
    conn.query_row(
        "SELECT name, payload, question_count, created_at, updated_at FROM study_banks WHERE bank_key = ?1",
        params![bank_key],
        |r| {
            Ok(StoredBank {
                name: r.get(0)?,
                payload: r.get(1)?,
                question_count: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
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
    Ok(Json(serde_json::json!({
        "bankKey": bank_key,
        "questionCount": question_count,
        "updatedAt": now,
    })))
}

pub async fn banks_list_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, Err> {
    let conn = lock_conn(&state)?;
    let mut stmt = conn
        .prepare(
            "SELECT bank_key, name, question_count, created_at, updated_at
             FROM study_banks ORDER BY updated_at DESC, bank_key ASC LIMIT ?1",
        )
        .map_err(|e| err_msg(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e))?;
    let banks: Vec<serde_json::Value> = stmt
        .query_map(params![MAX_BANKS_LISTED], |r| {
            Ok(serde_json::json!({
                "bankKey": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "questionCount": r.get::<_, i64>(2)?,
                "createdAt": r.get::<_, i64>(3)?,
                "updatedAt": r.get::<_, i64>(4)?,
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
            "questionKey": qkey,
            "sessionQuestionId": format!("{session}#{qkey}"),
            "questionSnapshot": { "id": qkey, "stem": format!("stem of {qkey}") },
            "userAnswer": "A",
            "correctAnswer": "A",
            "isCorrect": is_correct,
            "flagged": flagged,
            "subject": subject,
            "chapter": "ch1",
            "knowledgePoint": "kp1",
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
        let page = run_feed(&conn, "cli", None, None, None, 100).unwrap();
        assert_eq!(page.stored_cursor, 0);
        let seqs: Vec<i64> = page.attempts.iter().map(|a| a.seq).collect();
        assert_eq!(seqs, vec![1, 3, 5]);
        assert_eq!(page.next_cursor, 5);

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
        let page = run_feed(&conn, "cli", Some(3), None, None, 100).unwrap();
        assert_eq!(page.stored_cursor, 3);
        let seqs: Vec<i64> = page.attempts.iter().map(|a| a.seq).collect();
        assert_eq!(seqs, vec![5]);

        // subject filter
        let page = run_feed(&conn, "cli", None, Some("math"), None, 100).unwrap();
        let seqs: Vec<i64> = page.attempts.iter().map(|a| a.seq).collect();
        assert_eq!(seqs, vec![1, 3]);

        // ack stores the cursor monotonically; ahead-of-max is rejected
        assert_eq!(run_ack(&conn, "cli", 5).unwrap(), 5);
        assert_eq!(run_ack(&conn, "cli", 3).unwrap(), 5);
        let ahead = run_ack(&conn, "cli", 6).unwrap_err();
        assert_eq!(ahead.0, StatusCode::BAD_REQUEST);

        // after ack the feed is empty and nextCursor stays at the stored cursor
        let page = run_feed(&conn, "cli", None, None, None, 100).unwrap();
        assert!(page.attempts.is_empty());
        assert_eq!(page.stored_cursor, 5);
        assert_eq!(page.next_cursor, 5);

        // a separate consumer starts from 0
        let page = run_feed(&conn, "phone", None, None, None, 100).unwrap();
        assert_eq!(page.stored_cursor, 0);
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

    #[test]
    fn bank_validation_and_roundtrip() {
        let flat = json!({
            "bankKey": "b1",
            "name": "Bank One",
            "questions": [
                { "id": "q1", "type": "single_choice", "stem": "s", "options": ["A", "B", "C", "D", "E"], "answer": "A" },
                { "id": "q2", "type": "short_answer", "stem": "s", "answer": "x" }
            ]
        });
        let (key, name, payload, count) = validate_bank(&flat).unwrap();
        assert_eq!(key, "b1");
        assert_eq!(name, "Bank One");
        assert_eq!(count, 2);

        let nested = json!({ "bankKey": "b2", "bank": { "name": "Nested", "questions": [] } });
        assert!(validate_bank(&nested).is_ok());

        assert!(validate_bank(&json!({ "name": "N", "questions": [] })).is_err());
        assert!(validate_bank(&json!({ "bankKey": "b", "questions": [] })).is_err());
        assert!(validate_bank(&json!({ "bankKey": "b", "name": "N" })).is_err());

        let too_many = json!({
            "bankKey": "b3", "name": "N",
            "questions": [ { "type": "multi_choice", "options": ["1", "2", "3", "4", "5", "6"] } ]
        });
        assert!(validate_bank(&too_many).is_err());

        let unknown_type_many = json!({
            "bankKey": "b4", "name": "N",
            "questions": [ { "options": ["1", "2", "3", "4", "5", "6"] } ]
        });
        assert!(validate_bank(&unknown_type_many).is_err());

        // roundtrip storage: re-import bumps updated_at but keeps created_at
        let conn = test_conn();
        let payload_json = serde_json::to_string(&payload).unwrap();
        run_bank_import(&conn, &key, &name, &payload_json, count, 1111).unwrap();
        let stored = run_bank_get(&conn, "b1").unwrap().unwrap();
        assert_eq!(stored.name, "Bank One");
        assert_eq!(stored.question_count, 2);
        assert_eq!(stored.created_at, 1111);
        assert_eq!(stored.updated_at, 1111);

        run_bank_import(&conn, "b1", "Bank One v2", &payload_json, count, 2222).unwrap();
        let stored = run_bank_get(&conn, "b1").unwrap().unwrap();
        assert_eq!(stored.name, "Bank One v2");
        assert_eq!(stored.created_at, 1111);
        assert_eq!(stored.updated_at, 2222);

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
