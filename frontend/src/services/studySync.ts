/**
 * Local-first study sync service.
 *
 * Records durable per-attempt data to a user-configured self-hosted study API
 * (POST {baseUrl}/api/study/attempts/batch). Practice never blocks on the
 * network: attempts are queued in a bounded localStorage outbox and flushed
 * opportunistically (after each enqueue, on `online`) or manually from the
 * config screen. Failures simply stay pending.
 *
 * The access token is only ever attached to the Authorization header and is
 * never logged.
 */
import { ref } from 'vue'
import type { PracticeSession, PracticeSessionItem, Question } from '@exameow/shared'

export interface StudySyncConfig {
  baseUrl: string
  token: string
}

/** Content snapshot of the question as it was answered */
export interface StudyQuestionSnapshot {
  id: string
  stableKey?: string
  type: string
  stem: string
  options: string[]
  answer: string
  analysis: string
  difficulty?: string
  score?: number
  subject?: string
  chapter?: string
  knowledgePoint?: string
  tags?: string[]
  sourceMeta?: Record<string, unknown>
}

export interface StudyAttemptPayload {
  idempotencyKey: string
  sessionKey: string
  questionKey: string
  sessionQuestionId: string
  questionSnapshot: StudyQuestionSnapshot
  userAnswer: string | null
  correctAnswer: string
  isCorrect: boolean | null
  flagged: boolean
  subject: string
  chapter: string
  knowledgePoint: string
  submittedAt: number
}

export interface FlushResult {
  ok: boolean
  sent: number
  remaining: number
}

export interface ConnectionTestResult {
  ok: boolean
  message: string
}

const CONFIG_KEY = 'exameow-study-sync-config'
const DEVICE_KEY = 'exameow-study-sync-device'
const OUTBOX_KEY = 'exameow-study-sync-outbox'

/** Keep the outbox bounded so localStorage cannot grow without limit */
const MAX_OUTBOX_ENTRIES = 500
/** Attempts sent per HTTP batch request */
const BATCH_SIZE = 50
const REQUEST_TIMEOUT_MS = 15_000

/** Reactive state shared with the config screen */
export const pendingCount = ref(0)
export const syncing = ref(false)

function safeGet(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function safeSet(key: string, value: string) {
  try {
    localStorage.setItem(key, value)
  } catch {}
}

function randomId(): string {
  return Date.now().toString(36) + '-' + Math.random().toString(36).slice(2, 10)
}

function loadOutbox(): StudyAttemptPayload[] {
  try {
    const raw = safeGet(OUTBOX_KEY)
    const parsed = raw ? JSON.parse(raw) : []
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

function persistOutbox(attempts: StudyAttemptPayload[]) {
  safeSet(OUTBOX_KEY, JSON.stringify(attempts))
  pendingCount.value = attempts.length
}

persistOutbox(loadOutbox())

// ─── Config ────────────────────────────────────────────────────────────────

function normalizeBaseUrl(baseUrl: string): string {
  return baseUrl.trim().replace(/\/+$/, '')
}

export function loadStudySyncConfig(): StudySyncConfig {
  try {
    const raw = safeGet(CONFIG_KEY)
    const parsed = raw ? JSON.parse(raw) : null
    if (parsed && typeof parsed === 'object') {
      return {
        baseUrl: typeof parsed.baseUrl === 'string' ? parsed.baseUrl : '',
        token: typeof parsed.token === 'string' ? parsed.token : '',
      }
    }
  } catch {}
  return { baseUrl: '', token: '' }
}

export function saveStudySyncConfig(config: StudySyncConfig) {
  safeSet(CONFIG_KEY, JSON.stringify({
    baseUrl: normalizeBaseUrl(config.baseUrl),
    token: config.token.trim(),
  }))
}

export function isStudySyncConfigured(): boolean {
  return loadStudySyncConfig().baseUrl !== ''
}

/** Persistent device identifier (kept for future multi-device sync; not sent in the current contract) */
export function getDeviceId(): string {
  let deviceId = safeGet(DEVICE_KEY)
  if (!deviceId) {
    deviceId = typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? crypto.randomUUID()
      : randomId()
    safeSet(DEVICE_KEY, deviceId)
  }
  return deviceId
}

// ─── Payload building ───────────────────────────────────────────────────────

function buildSnapshot(q: Question): StudyQuestionSnapshot {
  return {
    id: q.id,
    stableKey: q.stableKey,
    type: q.type,
    stem: q.stem,
    options: q.options,
    answer: q.answer,
    analysis: q.analysis,
    difficulty: q.difficulty,
    score: q.score,
    subject: q.subject,
    chapter: q.chapter,
    knowledgePoint: q.knowledgePoint,
    tags: q.tags,
    sourceMeta: q.sourceMeta,
  }
}

/**
 * questionKey: stable identity of the underlying question.
 * Prefer question.stableKey; otherwise strip the per-session `-sN` suffix
 * added by the practice store.
 */
function buildQuestionKey(q: Question): string {
  if (q.stableKey) return q.stableKey
  return q.id.replace(/-s\d+$/, '')
}

/**
 * Queue a finished (or updated) attempt for a session item and trigger a
 * non-blocking flush. Re-submitting the same item (e.g. flag toggled or
 * self-check graded after submit) reuses its idempotencyKey so the server
 * updates the same history row instead of creating duplicates.
 */
export function recordStudyAttempt(session: PracticeSession, item: PracticeSessionItem) {
  const sessionKey = session.sessionKey
  const attemptId = item.attemptId
  if (!sessionKey || !attemptId) return
  const q = item.question
  enqueueAttempt({
    idempotencyKey: `${sessionKey}-${attemptId}`,
    sessionKey,
    questionKey: buildQuestionKey(q),
    sessionQuestionId: q.id,
    questionSnapshot: buildSnapshot(q),
    userAnswer: item.userAnswer,
    correctAnswer: q.answer,
    isCorrect: item.isCorrect,
    flagged: item.flagged === true,
    subject: q.subject ?? '',
    chapter: q.chapter ?? '',
    knowledgePoint: q.knowledgePoint ?? '',
    submittedAt: item.submittedAt ?? Date.now(),
  })
  triggerFlush()
}

// ─── Outbox ────────────────────────────────────────────────────────────────

/** Upsert by idempotencyKey; drop oldest entries when the bound is exceeded */
export function enqueueAttempt(attempt: StudyAttemptPayload) {
  const outbox = loadOutbox()
  const existingIndex = outbox.findIndex(a => a.idempotencyKey === attempt.idempotencyKey)
  if (existingIndex >= 0) {
    outbox[existingIndex] = attempt
  } else {
    outbox.push(attempt)
    if (outbox.length > MAX_OUTBOX_ENTRIES) {
      outbox.splice(0, outbox.length - MAX_OUTBOX_ENTRIES)
    }
  }
  persistOutbox(outbox)
  ensureOnlineListener()
}

// ─── Flush ─────────────────────────────────────────────────────────────────

function joinUrl(baseUrl: string, path: string): string {
  return `${normalizeBaseUrl(baseUrl)}/${path}`
}

function authHeaders(token: string, json = true): Record<string, string> {
  const headers: Record<string, string> = {}
  if (json) headers['Content-Type'] = 'application/json'
  if (token) headers['Authorization'] = `Bearer ${token}`
  return headers
}

/** POST one batch; returns the HTTP status, or null on network error/timeout */
async function postBatch(
  baseUrl: string,
  token: string,
  attempts: StudyAttemptPayload[],
): Promise<number | null> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS)
  try {
    const res = await fetch(joinUrl(baseUrl, 'api/study/attempts/batch'), {
      method: 'POST',
      headers: authHeaders(token),
      body: JSON.stringify({ attempts }),
      signal: controller.signal,
    })
    return res.status
  } catch {
    return null
  } finally {
    clearTimeout(timer)
  }
}

/** GET /api/study/health; returns the HTTP status, or null on network error/timeout */
async function getHealth(baseUrl: string, token: string): Promise<number | null> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS)
  try {
    const res = await fetch(joinUrl(baseUrl, 'api/study/health'), {
      headers: authHeaders(token, false),
      signal: controller.signal,
    })
    return res.status
  } catch {
    return null
  } finally {
    clearTimeout(timer)
  }
}

async function doFlush(): Promise<FlushResult> {
  const config = loadStudySyncConfig()
  let sent = 0
  if (config.baseUrl) {
    syncing.value = true
    try {
      for (;;) {
        const outbox = loadOutbox()
        if (outbox.length === 0) break
        const batch = outbox.slice(0, BATCH_SIZE)
        const status = await postBatch(config.baseUrl, config.token, batch)
        if (status === null) {
          console.warn('[studySync] flush stopped: network error, attempts stay pending')
          break
        }
        if (status < 200 || status >= 300) {
          console.warn(`[studySync] flush stopped: HTTP ${status}, attempts stay pending`)
          break
        }
        // Batch accepted — drop exactly what the server received. Attempts
        // that were upserted while the request was in flight (e.g. a flag
        // toggled) differ from the sent payload and stay queued so the next
        // batch updates the same server row via the idempotencyKey.
        const sentPayloads = new Set(batch.map(a => JSON.stringify(a)))
        const current = loadOutbox().filter(a => !sentPayloads.has(JSON.stringify(a)))
        persistOutbox(current)
        sent += batch.length
      }
    } finally {
      syncing.value = false
    }
  }
  const remaining = loadOutbox().length
  pendingCount.value = remaining
  return { ok: remaining === 0, sent, remaining }
}

let flushChain: Promise<FlushResult> = Promise.resolve({ ok: true, sent: 0, remaining: 0 })

/** Serialized, non-blocking flush. Failures leave attempts pending. */
export function flushAttempts(): Promise<FlushResult> {
  flushChain = flushChain.then(doFlush, doFlush)
  return flushChain
}

/** Fire-and-forget flush used after submits and config changes */
export function triggerFlush() {
  void flushAttempts()
}

/**
 * Opportunistic flush when connectivity returns. No polling: the listener is
 * registered once, lazily, on first enqueue.
 */
let onlineListenerBound = false
function ensureOnlineListener() {
  if (onlineListenerBound) return
  onlineListenerBound = true
  window.addEventListener('online', () => {
    if (isStudySyncConfigured()) triggerFlush()
  })
}

// ─── Connection test ───────────────────────────────────────────────────────

/**
 * Test connectivity against GET {baseUrl}/api/study/health using the given
 * (possibly unsaved) credentials. Never logs or echoes the token.
 */
export async function testStudyConnection(baseUrl: string, token: string): Promise<ConnectionTestResult> {
  if (!normalizeBaseUrl(baseUrl)) {
    return { ok: false, message: 'no base URL' }
  }
  const status = await getHealth(baseUrl, token)
  if (status === null) {
    return { ok: false, message: 'network error' }
  }
  return { ok: status >= 200 && status < 300, message: `HTTP ${status}` }
}
