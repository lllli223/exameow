//! Revisioned cross-device practice progress.
//!
//! Keeps snapshots separate from the immutable study attempt/event tables.
//! All routes are mounted below study::study_auth; every write uses
//! compare-and-swap against an integer revision to prevent silent overwrites.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::routes::AppState;

const MAX_BANK_KEY: usize = 512;
const MAX_DEVICE_ID: usize = 128;
const MAX_SESSION_BYTES: usize = 8 * 1024 * 1024;
const MAX_QUESTIONS: usize = 5000;

pub fn init_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS study_progress (
            bank_key TEXT PRIMARY KEY,
            revision INTEGER NOT NULL,
            session_json TEXT,
            device_id TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS study_progress_history (
            bank_key TEXT NOT NULL,
            revision INTEGER NOT NULL,
            session_json TEXT,
            device_id TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (bank_key, revision)
        );
        CREATE INDEX IF NOT EXISTS idx_study_progress_updated ON study_progress(updated_at);",
    ).map_err(|e| e.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

fn validate_bank_key(key: &str) -> Result<(), Response> {
    if key.trim().is_empty() || key.len() > MAX_BANK_KEY {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_bank_key"));
    }
    Ok(())
}

fn validate_revision(revision: i64) -> Result<(), Response> {
    if !(0..i64::MAX).contains(&revision) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_revision"));
    }
    Ok(())
}

fn validate_session(session: &Value, bank_key: &str) -> Result<String, Response> {
    let Some(obj) = session.as_object() else {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_session"));
    };
    let Some(bank_id) = obj.get("bankId").and_then(Value::as_str) else {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_bank_id"));
    };
    if bank_id.trim().is_empty() || bank_id.len() > MAX_BANK_KEY {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_bank_id"));
    }
    if obj.get("bankKey").and_then(Value::as_str).is_some_and(|key| key != bank_key) {
        return Err(error(StatusCode::BAD_REQUEST, "bank_key_mismatch"));
    }
    if !matches!(obj.get("mode").and_then(Value::as_str),
        Some("sequential" | "random" | "mock")) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_practice_mode"));
    }
    let Some(questions) = obj.get("questions").and_then(Value::as_array) else {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_questions"));
    };
    if questions.is_empty() || questions.len() > MAX_QUESTIONS
        || questions.iter().any(|item| {
            item.get("question").and_then(|q| q.get("id")).and_then(Value::as_str).is_none()
        })
    {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_questions"));
    }
    if !obj.get("currentIndex").and_then(Value::as_u64)
        .is_some_and(|idx| (idx as usize) < questions.len())
        || !obj.get("startedAt").and_then(Value::as_i64).is_some_and(|ts| ts > 0)
    {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_session_position"));
    }
    let serialized = serde_json::to_string(session)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_session_json"))?;
    if serialized.len() > MAX_SESSION_BYTES {
        return Err(error(StatusCode::PAYLOAD_TOO_LARGE, "progress_too_large"));
    }
    Ok(serialized)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressWrite {
    expected_revision: i64,
    device_id: String,
    session: Option<Value>,
}

fn validate_device(device: &str) -> Result<(), Response> {
    if device.trim().is_empty() || device.len() > MAX_DEVICE_ID {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_device_id"));
    }
    Ok(())
}

fn db_error(_: rusqlite::Error) -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "db_error")
}

fn lock_error() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "db_lock")
}

#[derive(Clone)]
struct Snapshot {
    revision: i64,
    session: Option<String>,
    device_id: String,
    updated_at: i64,
}

fn read_current(conn: &Connection, bank_key: &str) -> Result<Option<Snapshot>, rusqlite::Error> {
    conn.query_row(
        "SELECT revision, session_json, device_id, updated_at FROM study_progress WHERE bank_key = ?1",
        params![bank_key],
        |r| Ok(Snapshot {
            revision: r.get(0)?,
            session: r.get(1)?,
            device_id: r.get(2)?,
            updated_at: r.get(3)?,
        }),
    ).optional()
}

fn public_row(bank_key: &str, row: &Snapshot) -> Value {
    json!({
        "bankKey": bank_key,
        "revision": row.revision,
        "updatedAt": row.updated_at,
        "deviceId": row.device_id,
        "deleted": row.session.is_none(),
    })
}

pub async fn list_handler(State(state): State<Arc<AppState>>) -> Response {
    let Ok(conn) = state.relay.conn.lock() else { return lock_error(); };
    let Ok(mut stmt) = conn.prepare(
        "SELECT bank_key, revision, session_json IS NULL, updated_at, device_id
         FROM study_progress ORDER BY updated_at DESC LIMIT 1000",
    ) else { return error(StatusCode::INTERNAL_SERVER_ERROR, "db_error"); };
    let rows = stmt.query_map([], |r| {
        Ok(json!({
            "bankKey": r.get::<_, String>(0)?,
            "revision": r.get::<_, i64>(1)?,
            "deleted": r.get::<_, bool>(2)?,
            "updatedAt": r.get::<_, i64>(3)?,
            "deviceId": r.get::<_, String>(4)?,
        }))
    });
    match rows {
        Ok(rows) => {
            let collected: Result<Vec<Value>, _> = rows.collect();
            match collected {
                Ok(items) => Json(json!({ "progress": items })).into_response(),
                Err(err) => db_error(err),
            }
        },
        Err(err) => db_error(err),
    }
}

pub async fn get_handler(
    State(state): State<Arc<AppState>>,
    Path(bank_key): Path<String>,
) -> Response {
    if let Err(resp) = validate_bank_key(&bank_key) { return resp; }
    let Ok(conn) = state.relay.conn.lock() else { return lock_error(); };
    match read_current(&conn, &bank_key) {
        Ok(Some(row)) => {
            let session: Option<Value> = row.session.as_deref()
                .and_then(|str_value| serde_json::from_str(str_value).ok());
            Json(json!({
                "bankKey": bank_key,
                "revision": row.revision,
                "session": session,
                "deviceId": row.device_id,
                "updatedAt": row.updated_at,
                "deleted": row.session.is_none(),
            })).into_response()
        }
        Ok(None) => error(StatusCode::NOT_FOUND, "no_progress"),
        Err(err) => db_error(err),
    }
}

pub async fn put_handler(
    State(state): State<Arc<AppState>>,
    Path(bank_key): Path<String>,
    Json(body): Json<ProgressWrite>,
) -> Response {
    if let Err(resp) = validate_bank_key(&bank_key) { return resp; }
    if let Err(resp) = validate_revision(body.expected_revision) { return resp; }
    if let Err(resp) = validate_device(&body.device_id) { return resp; }
    let Some(value) = body.session.as_ref() else {
        return error(StatusCode::BAD_REQUEST, "missing_session");
    };
    let json_text = match validate_session(value, &bank_key) {
        Ok(text) => text,
        Err(resp) => return resp,
    };
    commit(&state, &bank_key, body.expected_revision, &body.device_id, Some(json_text))
}

pub async fn delete_handler(
    State(state): State<Arc<AppState>>,
    Path(bank_key): Path<String>,
    Json(body): Json<ProgressWrite>,
) -> Response {
    if let Err(resp) = validate_bank_key(&bank_key) { return resp; }
    if let Err(resp) = validate_revision(body.expected_revision) { return resp; }
    if let Err(resp) = validate_device(&body.device_id) { return resp; }
    commit(&state, &bank_key, body.expected_revision, &body.device_id, None)
}

fn commit(state: &AppState, bank_key: &str, expected: i64, device: &str, snapshot: Option<String>) -> Response {
    let Ok(mut conn) = state.relay.conn.lock() else { return lock_error(); };
    let Ok(tx) = conn.transaction() else { return error(StatusCode::INTERNAL_SERVER_ERROR, "db_error"); };
    let current = match read_current(&tx, bank_key) {
        Ok(row) => row,
        Err(err) => return db_error(err),
    };
    let actual = current.as_ref().map(|row| row.revision).unwrap_or(0);
    if actual != expected {
        let row = current.as_ref()
            .map(|current| public_row(bank_key, current))
            .unwrap_or(json!({ "revision": 0, "updatedAt": 0, "deleted": true }));
        return (StatusCode::CONFLICT, Json(json!({
            "error": "revision_conflict",
            "current": row,
        }))).into_response();
    }
    let revision = actual + 1;
    let now = now_ms();
    if let Err(err) = tx.execute(
        "INSERT INTO study_progress(bank_key, revision, session_json, device_id, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(bank_key) DO UPDATE SET
           revision=excluded.revision, session_json=excluded.session_json,
           device_id=excluded.device_id, updated_at=excluded.updated_at",
        params![bank_key, revision, snapshot, device, now],
    ) {
        return db_error(err);
    }
    if let Err(err) = tx.execute(
        "INSERT INTO study_progress_history(bank_key, revision, session_json, device_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![bank_key, revision, snapshot, device, now],
    ) {
        return db_error(err);
    }
    // Keep a bounded history for recovery if a user explicitly resolves a conflict by overwriting.
    if let Err(err) = tx.execute(
        "DELETE FROM study_progress_history
         WHERE bank_key = ?1 AND revision <= ?2",
        params![bank_key, revision - 20],
    ) {
        return db_error(err);
    }
    if let Err(err) = tx.commit() { return db_error(err); }
    Json(json!({ "bankKey": bank_key, "revision": revision, "updatedAt": now, "deleted": snapshot.is_none() })).into_response()
}
