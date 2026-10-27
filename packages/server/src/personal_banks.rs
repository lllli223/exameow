//! Revisioned user-owned question-bank synchronization.
//!
//! These banks are separate from operator-imported `study_banks`. A personal
//! bank's original ID is kept unchanged on every device; study_attempts and
//! study_progress keep referring to exactly the same logical bank.
//! Optimistic concurrency prevents a stale device from silently overwriting a
//! bank or reviving a deleted one. The study bearer token protects all routes.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use crate::routes::AppState;

const MAX_BANK_ID: usize = 512;
const MAX_DEVICE_ID: usize = 128;
const MAX_BANK_BYTES: usize = 10 * 1024 * 1024;
const MAX_QUESTIONS: usize = 5000;
const MAX_NAME: usize = 500;

pub fn init_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS study_personal_banks (
           bank_id TEXT PRIMARY KEY,
           revision INTEGER NOT NULL,
           bank_json TEXT,
           name TEXT NOT NULL DEFAULT '',
           question_count INTEGER NOT NULL DEFAULT 0,
           content_hash TEXT NOT NULL DEFAULT '',
           device_id TEXT NOT NULL DEFAULT '',
           updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS study_personal_bank_history (
           bank_id TEXT NOT NULL,
           revision INTEGER NOT NULL,
           bank_json TEXT,
           device_id TEXT NOT NULL,
           updated_at INTEGER NOT NULL,
           PRIMARY KEY (bank_id, revision)
         );
         CREATE INDEX IF NOT EXISTS idx_personal_banks_updated
           ON study_personal_banks(updated_at);",
    ).map_err(|err| err.to_string())
}

fn err(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}
fn db_error(_: rusqlite::Error) -> Response { err(StatusCode::INTERNAL_SERVER_ERROR, "db_error") }
fn lock_error() -> Response { err(StatusCode::INTERNAL_SERVER_ERROR, "db_lock") }
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64).unwrap_or(0)
}
fn hash(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}
fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= MAX_BANK_ID
        && !id.contains('/') && !id.contains('\\')
}
fn check_id(id: &str) -> Result<(), Response> {
    if !valid_id(id) { return Err(err(StatusCode::BAD_REQUEST, "invalid_bank_id")); }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BankWrite {
    expected_revision: i64,
    device_id: String,
    #[serde(default)]
    bank: Option<Value>,
}

fn validate_write(body: &BankWrite) -> Result<(), Response> {
    if !(0..i64::MAX).contains(&body.expected_revision) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_revision"));
    }
    if body.device_id.trim().is_empty() || body.device_id.len() > MAX_DEVICE_ID {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_device_id"));
    }
    Ok(())
}

struct ValidBank { text: String, name: String, count: usize, content_hash: String }
fn validate_bank(bank: &Value, id: &str) -> Result<ValidBank, Response> {
    let Some(obj) = bank.as_object() else {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_bank"));
    };
    if obj.get("id").and_then(Value::as_str) != Some(id) {
        return Err(err(StatusCode::BAD_REQUEST, "bank_id_mismatch"));
    }
    let Some(name) = obj.get("name").and_then(Value::as_str) else {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_bank_name"));
    };
    if name.trim().is_empty() || name.len() > MAX_NAME {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_bank_name"));
    }
    if !matches!(obj.get("source").and_then(Value::as_str),
        Some("ai-generated" | "csv-import" | "xlsx-import")) {
        // Prevent official study-server banks being overwritten by personal sync.
        return Err(err(StatusCode::BAD_REQUEST, "not_a_personal_bank"));
    }
    if obj.get("createdAt").and_then(Value::as_u64).is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_creation_date"));
    }
    let Some(questions) = obj.get("questions").and_then(Value::as_array) else {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_questions"));
    };
    if questions.is_empty() || questions.len() > MAX_QUESTIONS {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_questions"));
    }
    // Unlike the official study bank schema, retain user imports as-is:
    // their answer/option normalization might differ by import format.
    if questions.iter().any(|q| {
        q.get("id").and_then(Value::as_str).is_none_or(str::is_empty)
            || q.get("type").and_then(Value::as_str).is_none()
            || q.get("stem").and_then(Value::as_str).is_none()
            || q.get("answer").and_then(Value::as_str).is_none()
    }) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_question"));
    }
    let text = serde_json::to_string(bank)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid_bank"))?;
    if text.len() > MAX_BANK_BYTES {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "bank_too_large"));
    }
    Ok(ValidBank { content_hash: hash(&text), text, name: name.into(), count: questions.len() })
}

struct Stored {
    revision: i64,
    bank_json: Option<String>,
    name: String,
    count: i64,
    hash: String,
    device_id: String,
    updated_at: i64,
}
fn read_bank(conn: &Connection, id: &str) -> rusqlite::Result<Option<Stored>> {
    conn.query_row(
        "SELECT revision, bank_json, name, question_count, content_hash, device_id, updated_at
         FROM study_personal_banks WHERE bank_id = ?1",
        params![id],
        |r| Ok(Stored {
            revision: r.get(0)?, bank_json: r.get(1)?, name: r.get(2)?,
            count: r.get(3)?, hash: r.get(4)?, device_id: r.get(5)?, updated_at: r.get(6)?,
        }),
    ).optional()
}
fn summary(id: &str, stored: &Stored) -> Value {
    json!({
        "bankId": id, "revision": stored.revision,
        "name": stored.name, "questionCount": stored.count,
        "contentHash": stored.hash, "deviceId": stored.device_id,
        "updatedAt": stored.updated_at, "deleted": stored.bank_json.is_none()
    })
}

pub async fn list_handler(State(state): State<Arc<AppState>>) -> Response {
    let Ok(conn) = state.relay.conn.lock() else { return lock_error(); };
    let Ok(mut stmt) = conn.prepare(
        "SELECT bank_id, revision, bank_json IS NULL, name, question_count,
                content_hash, device_id, updated_at
         FROM study_personal_banks ORDER BY updated_at DESC LIMIT 1000"
    ) else { return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error"); };
    let result = stmt.query_map([], |r| Ok(json!({
        "bankId": r.get::<_, String>(0)?,
        "revision": r.get::<_, i64>(1)?,
        "deleted": r.get::<_, bool>(2)?,
        "name": r.get::<_, String>(3)?,
        "questionCount": r.get::<_, i64>(4)?,
        "contentHash": r.get::<_, String>(5)?,
        "deviceId": r.get::<_, String>(6)?,
        "updatedAt": r.get::<_, i64>(7)?,
    })));
    match result {
        Ok(rows) => match rows.collect::<rusqlite::Result<Vec<Value>>>() {
            Ok(banks) => Json(json!({ "banks": banks })).into_response(),
            Err(error) => db_error(error),
        }
        Err(error) => db_error(error),
    }
}

pub async fn get_handler(
    State(state): State<Arc<AppState>>, Path(id): Path<String>,
) -> Response {
    if let Err(response) = check_id(&id) { return response; }
    let Ok(conn) = state.relay.conn.lock() else { return lock_error(); };
    match read_bank(&conn, &id) {
        Ok(Some(stored)) => {
            let payload = stored.bank_json.as_ref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
            Json(json!({ "bank": payload, "bankId": id, "revision": stored.revision,
                "deleted": stored.bank_json.is_none(), "contentHash": stored.hash,
                "updatedAt": stored.updated_at, "deviceId": stored.device_id })).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "personal_bank_not_found"),
        Err(error) => db_error(error),
    }
}

pub async fn put_handler(
    State(state): State<Arc<AppState>>, Path(id): Path<String>,
    Json(body): Json<BankWrite>,
) -> Response {
    if let Err(response) = check_id(&id) { return response; }
    if let Err(response) = validate_write(&body) { return response; }
    let Some(bank) = body.bank.as_ref() else {
        return err(StatusCode::BAD_REQUEST, "missing_bank");
    };
    let valid = match validate_bank(bank, &id) {
        Ok(value) => value, Err(response) => return response,
    };
    commit(&state, &id, body.expected_revision, &body.device_id, Some(valid))
}

pub async fn delete_handler(
    State(state): State<Arc<AppState>>, Path(id): Path<String>,
    Json(body): Json<BankWrite>,
) -> Response {
    if let Err(response) = check_id(&id) { return response; }
    if let Err(response) = validate_write(&body) { return response; }
    commit(&state, &id, body.expected_revision, &body.device_id, None)
}

fn commit(state: &AppState, id: &str, expected: i64, device: &str,
    payload: Option<ValidBank>,
) -> Response {
    let Ok(mut conn) = state.relay.conn.lock() else { return lock_error(); };
    let Ok(tx) = conn.transaction() else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error");
    };
    let stored = match read_bank(&tx, id) {
        Ok(row) => row, Err(error) => return db_error(error),
    };
    let current_revision = stored.as_ref().map(|row| row.revision).unwrap_or(0);
    if current_revision != expected {
        return (StatusCode::CONFLICT, Json(json!({
            "error": "revision_conflict",
            "current": stored.as_ref().map(|row| summary(id, row))
                .unwrap_or(json!({"bankId": id, "revision": 0, "deleted": true}))
        }))).into_response();
    }
    let revision = current_revision + 1;
    let now = now_ms();
    let (raw, name, count, content_hash) = match payload {
        Some(value) => (Some(value.text), value.name, value.count as i64, value.content_hash),
        None => (None, String::new(), 0, String::new()),
    };
    if let Err(error) = tx.execute(
        "INSERT INTO study_personal_banks
         (bank_id, revision, bank_json, name, question_count, content_hash, device_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(bank_id) DO UPDATE SET
            revision=excluded.revision, bank_json=excluded.bank_json, name=excluded.name,
            question_count=excluded.question_count, content_hash=excluded.content_hash,
            device_id=excluded.device_id, updated_at=excluded.updated_at",
        params![id, revision, raw, name, count, content_hash, device, now],
    ) { return db_error(error); }
    if let Err(error) = tx.execute(
        "INSERT INTO study_personal_bank_history
         (bank_id, revision, bank_json, device_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, revision, raw, device, now],
    ) { return db_error(error); }
    if let Err(error) = tx.execute(
        "DELETE FROM study_personal_bank_history
         WHERE bank_id=?1 AND revision <= ?2",
        params![id, revision - 20],
    ) { return db_error(error); }
    if let Err(error) = tx.commit() { return db_error(error); }
    Json(json!({
        "bankId": id, "revision": revision, "updatedAt": now,
        "contentHash": content_hash, "deleted": raw.is_none()
    })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_bank_validation_allows_noncanonical_answers_and_preserves_ids() {
        let value = json!({
            "id": "legacy-csv-1699", "name": "测试导入题库",
            "source": "csv-import", "createdAt": 1699000000000_u64,
            "questions": [{
                "id": "csv-1", "type": "true_false", "stem": "Hello?",
                "options": [], "answer": "正确", "analysis": "解析"
            }]
        });
        let good = validate_bank(&value, "legacy-csv-1699").unwrap();
        assert_eq!(good.count, 1);
        assert!(good.text.contains("正确"));
        assert!(validate_bank(&value, "different-id").is_err());
    }

    #[test]
    fn never_accepts_official_bank_as_personal_bank() {
        let value = json!({
            "id": "study-bank:os", "name": "OS", "source": "server-sync",
            "createdAt": 1000,
            "questions": [{"id":"a", "type":"single_choice", "stem":"q", "answer":"A"}]
        });
        assert!(validate_bank(&value, "study-bank:os").is_err());
    }
}
