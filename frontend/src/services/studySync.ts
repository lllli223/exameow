/**
 * Local-first study sync service.
 *
 * Records durable per-attempt data to a user-configured self-hosted study API
 * (POST {baseUrl}/api/study/attempts/batch). Practice never blocks on the
 * network: attempts are queued in a bounded localStorage outbox and flushed
 * opportunistically (after enqueue, on reconnect, periodic retry, and reload) or manually from the
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
  deviceId: string
  bankKey: string
  questionKey: string
  originalQuestionId: string
  sessionQuestionId: string
  questionSnapshot: StudyQuestionSnapshot
  userAnswer: string | null
  correctAnswer: string
  isCorrect: boolean | null
  flagged: boolean
  subject: string
  chapter: string
  knowledgePoint: string
  durationMs: number
  submittedAt: number
}

export interface StudySessionFinishPayload {
  sessionKey: string
  startedAt: number
  finishedAt: number
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

export interface StudyBankSummary {
  bankKey: string
  name: string
  contentHash?: string
  questionCount: number
  createdAt: number
  updatedAt: number
}

export interface StudyBankQuestionDocument {
  id?: string
  stableKey?: string
  type: Question['type']
  stem: string
  options?: string[]
  answer: string
  analysis: string
  subject?: string
  chapter?: string
  knowledgePoint?: string
  difficulty?: Question['difficulty']
  tags?: string[]
  sourceMeta?: Record<string, unknown>
}

export interface StudyBankDocument {
  schemaVersion: number
  key: string
  name: string
  version: number
  metadata: { exam: string; outlineVersion: string; subject: string }
  questions: StudyBankQuestionDocument[]
  subject?: string
  chapter?: string
  tags?: string[]
  sourceMeta?: Record<string, unknown>
}

export interface StudyBankEnvelope extends StudyBankSummary {
  bank: StudyBankDocument
}

const CONFIG_KEY = 'exameow-study-sync-config'
const DEVICE_KEY = 'exameow-study-sync-device'
const OUTBOX_KEY = 'exameow-study-sync-outbox'
const SESSION_OUTBOX_KEY = 'exameow-study-sync-session-outbox'

/** Keep the outbox bounded so localStorage cannot grow without limit */
const MAX_OUTBOX_ENTRIES = 500
/** Attempts sent per HTTP batch request */
const BATCH_SIZE = 50
const REQUEST_TIMEOUT_MS = 15_000
const RETRY_INTERVAL_MS = 30_000

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
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return crypto.randomUUID()
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

function loadSessionOutbox(): StudySessionFinishPayload[] {
  try {
    const raw = safeGet(SESSION_OUTBOX_KEY)
    const parsed = raw ? JSON.parse(raw) : []
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

function refreshPendingCount() {
  pendingCount.value = loadOutbox().length + loadSessionOutbox().length
}

function persistOutbox(attempts: StudyAttemptPayload[]) {
  safeSet(OUTBOX_KEY, JSON.stringify(attempts))
  refreshPendingCount()
}

function persistSessionOutbox(sessions: StudySessionFinishPayload[]) {
  safeSet(SESSION_OUTBOX_KEY, JSON.stringify(sessions))
  refreshPendingCount()
}

refreshPendingCount()

// ─── Config ────────────────────────────────────────────────────────────────

function normalizeBaseUrl(baseUrl: string): string {
  return baseUrl.trim().replace(/\/+$/, '')
}

function defaultStudyBaseUrl(): string {
  if (typeof window === 'undefined') return ''
  return window.location.protocol === 'http:' || window.location.protocol === 'https:'
    ? window.location.origin
    : ''
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
  return { baseUrl: defaultStudyBaseUrl(), token: '' }
}

export function saveStudySyncConfig(config: StudySyncConfig) {
  safeSet(CONFIG_KEY, JSON.stringify({
    baseUrl: normalizeBaseUrl(config.baseUrl),
    token: config.token.trim(),
  }))
}

export function isStudySyncConfigured(): boolean {
  const config = loadStudySyncConfig()
  return config.baseUrl !== '' && config.token !== ''
}

/** Persistent device identifier used to distinguish Android/Web/Desktop attempt sources. */
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
 * questionKey: stable identity of the underlying question, namespaced by
 * the logical bank key so source-local ids cannot collide across banks.
 */
function buildQuestionKey(session: PracticeSession, q: Question): string {
  const bankKey = session.bankKey || session.bankId
  const originalQuestionId = q.id.replace(/-s\d+$/, '')
  const stableQuestionId = q.stableKey || originalQuestionId
  return `${bankKey}:${stableQuestionId}`
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
    deviceId: getDeviceId(),
    bankKey: session.bankKey || session.bankId,
    questionKey: buildQuestionKey(session, q),
    originalQuestionId: q.id.replace(/-s\d+$/, ''),
    sessionQuestionId: q.id,
    questionSnapshot: buildSnapshot(q),
    userAnswer: item.userAnswer,
    correctAnswer: q.answer,
    isCorrect: item.isCorrect,
    flagged: item.flagged === true,
    subject: q.subject ?? '',
    chapter: q.chapter ?? '',
    knowledgePoint: q.knowledgePoint ?? '',
    durationMs: Math.max(0, item.durationMs ?? 0),
    submittedAt: item.submittedAt ?? Date.now(),
  })
  triggerFlush()
}

// ─── Outbox ────────────────────────────────────────────────────────────────

/** Upsert by idempotencyKey; drop oldest entries when the bound is exceeded */
export function recordStudySessionFinish(session: PracticeSession) {
  if (!session.sessionKey || session.finishedAt == null) return
  const payload: StudySessionFinishPayload = {
    sessionKey: session.sessionKey,
    startedAt: session.startedAt,
    finishedAt: session.finishedAt,
  }
  const outbox = loadSessionOutbox()
  const index = outbox.findIndex(item => item.sessionKey === payload.sessionKey)
  if (index >= 0) outbox[index] = payload
  else outbox.push(payload)
  persistSessionOutbox(outbox.slice(-MAX_OUTBOX_ENTRIES))
  ensureOnlineListener()
  triggerFlush()
}

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

async function postSessionFinish(
  baseUrl: string,
  token: string,
  payload: StudySessionFinishPayload,
): Promise<number | null> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS)
  try {
    const res = await fetch(joinUrl(baseUrl, 'api/study/sessions/finish'), {
      method: 'POST',
      headers: authHeaders(token),
      body: JSON.stringify(payload),
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

      for (;;) {
        const sessionOutbox = loadSessionOutbox()
        if (sessionOutbox.length === 0) break
        const payload = sessionOutbox[0]!
        const status = await postSessionFinish(config.baseUrl, config.token, payload)
        if (status === null || status < 200 || status >= 300) {
          console.warn(`[studySync] session finish flush stopped: ${status === null ? 'network error' : `HTTP ${status}`}`)
          break
        }
        const current = loadSessionOutbox().filter(item => !(
          item.sessionKey === payload.sessionKey && item.finishedAt === payload.finishedAt
        ))
        persistSessionOutbox(current)
        sent += 1
      }
    } finally {
      syncing.value = false
    }
  }
  const remaining = loadOutbox().length + loadSessionOutbox().length
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

/** Opportunistic flush when connectivity returns. */
let onlineListenerBound = false
function ensureOnlineListener() {
  if (onlineListenerBound || typeof window === 'undefined') return
  onlineListenerBound = true
  window.addEventListener('online', () => {
    if (isStudySyncConfigured()) triggerFlush()
  })
}

/** Retry pending writes after transient failures even when the browser stays online. */
let retryTimerBound = false
function ensureRetryTimer() {
  if (retryTimerBound || typeof window === 'undefined') return
  retryTimerBound = true
  window.setInterval(() => {
    if (pendingCount.value > 0 && isStudySyncConfigured() && window.navigator.onLine) triggerFlush()
  }, RETRY_INTERVAL_MS)
}

// ─── Remote study banks ────────────────────────────────────────────────────

async function getStudyJson<T>(path: string): Promise<T> {
  const config = loadStudySyncConfig()
  if (!config.baseUrl) throw new Error('study sync is not configured')
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS)
  try {
    const res = await fetch(joinUrl(config.baseUrl, path), {
      headers: authHeaders(config.token, false),
      signal: controller.signal,
    })
    if (!res.ok) throw new Error(`study API HTTP ${res.status}`)
    return await res.json() as T
  } finally {
    clearTimeout(timer)
  }
}

export async function listRemoteStudyBanks(): Promise<StudyBankSummary[]> {
  const payload = await getStudyJson<{ banks?: StudyBankSummary[] }>('api/study/banks')
  return Array.isArray(payload.banks) ? payload.banks : []
}

export async function getRemoteStudyBank(bankKey: string): Promise<StudyBankEnvelope> {
  return getStudyJson<StudyBankEnvelope>(`api/study/banks/${encodeURIComponent(bankKey)}`)
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

// Recover pending offline writes after a page reload without waiting for another answer.
if (typeof window !== 'undefined') {
  ensureOnlineListener()
  ensureRetryTimer()
  if (isStudySyncConfigured() && pendingCount.value > 0 && window.navigator.onLine) triggerFlush()
}
