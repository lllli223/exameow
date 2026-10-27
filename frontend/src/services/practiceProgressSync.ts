/**
 * Revisioned, local-first resumable practice sync. The study token is reused
 * from studySync; no new credentials, polling, or third-party service.
 */
import type { PracticeSession } from '@exameow/shared'
import { getDeviceId, isStudySyncConfigured, loadStudySyncConfig } from './studySync'

export interface CloudProgressSummary {
  bankKey: string
  revision: number
  updatedAt: number
  deviceId: string
  deleted: boolean
}

export interface CloudProgressDocument extends CloudProgressSummary {
  session: PracticeSession | null
}

export class ProgressSyncError extends Error {
  constructor(public readonly status: number, public readonly code: string) {
    super('Practice progress sync: ' + code + ' (' + status + ')')
  }
}

const META_KEY = 'exameow-practice-progress-meta'
const DELETED_KEY = 'exameow-practice-progress-deleted'
const BACKUP_PREFIX = 'exameow-practice-progress-backup:'
const TIMEOUT_MS = 20_000

export interface CloudProgressMeta {
  revision: number
  /** Fingerprint of the latest local content successfully acknowledged by the server. */
  fingerprint: string
}

function getStorage(key: string): string | null {
  try { return localStorage.getItem(key) } catch { return null }
}

function setStorage(key: string, content: string) {
  try { localStorage.setItem(key, content) } catch (error) {
    console.warn('[practice-progress] Unable to save local cloud sync metadata:', error)
  }
}

function readObject<T>(key: string): Record<string, T> {
  try {
    const parsed = JSON.parse(getStorage(key) || '{}')
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {}
  } catch {
    return {}
  }
}

export function loadCloudProgressMeta(): Record<string, CloudProgressMeta> {
  return readObject<CloudProgressMeta>(META_KEY)
}

export function saveCloudProgressMeta(values: Record<string, CloudProgressMeta>) {
  setStorage(META_KEY, JSON.stringify(values))
}

export function loadProgressDeleted(): Record<string, boolean> {
  return readObject<boolean>(DELETED_KEY)
}

export function setProgressDeleted(bankId: string, deleted: boolean) {
  const map = loadProgressDeleted()
  if (deleted) map[bankId] = true
  else delete map[bankId]
  setStorage(DELETED_KEY, JSON.stringify(map))
}

export function fingerprintSession(session: PracticeSession): string {
  // Active timers are local-only and must not trigger network uploads.
  const cleaned: PracticeSession = {
    ...session,
    questions: session.questions.map(item => ({ ...item, viewStartedAt: undefined })),
  }
  const value = JSON.stringify(cleaned)
  // Two independent 32-bit hashes + byte count; compact enough for localStorage.
  let a = 2166136261, b = 5381
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i)
    a = Math.imul(a ^ code, 16777619)
    b = Math.imul(b, 33) ^ code
  }
  return value.length + ':' + (a >>> 0).toString(16) + ':' + (b >>> 0).toString(16)
}

export function snapshotForCloud(session: PracticeSession): PracticeSession {
  // Clone to freeze an in-flight upload while the Pinia session keeps changing.
  return {
    ...session,
    questions: session.questions.map(item => ({ ...item, viewStartedAt: undefined })),
  }
}

export function backupPracticeProgress(bankId: string, source: string, session: PracticeSession) {
  const key = BACKUP_PREFIX + encodeURIComponent(bankId)
  // Keep one recoverable local copy of the discarded branch. Server also
  // retains 20 historic revisions for conflict resolution/recovery.
  setStorage(key, JSON.stringify({
    backedUpAt: Date.now(), source, session: snapshotForCloud(session),
  }))
}

async function request<T>(
  path: string,
  method = 'GET',
  body?: unknown,
): Promise<T> {
  if (!isStudySyncConfigured()) throw new ProgressSyncError(0, 'not_configured')
  const { baseUrl, token } = loadStudySyncConfig()
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), TIMEOUT_MS)
  try {
    const response = await fetch(baseUrl.replace(/\/+$/, '') + '/' + path, {
      method,
      headers: {
        Authorization: 'Bearer ' + token,
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
      signal: controller.signal,
    })
    const result = await response.json().catch(() => ({}))
    if (!response.ok) throw new ProgressSyncError(response.status, result.error || 'http_error')
    return result as T
  } finally {
    clearTimeout(timeout)
  }
}

export async function listCloudPracticeProgress(): Promise<CloudProgressSummary[]> {
  const result = await request<{ progress: CloudProgressSummary[] }>('api/study/progress')
  return Array.isArray(result.progress) ? result.progress : []
}

export function getCloudPracticeProgress(bankKey: string): Promise<CloudProgressDocument> {
  return request<CloudProgressDocument>('api/study/progress/' + encodeURIComponent(bankKey))
}

export function putCloudPracticeProgress(
  bankKey: string,
  session: PracticeSession,
  expectedRevision: number,
): Promise<CloudProgressSummary> {
  return request<CloudProgressSummary>(
    'api/study/progress/' + encodeURIComponent(bankKey), 'PUT',
    { expectedRevision, deviceId: getDeviceId(), session: snapshotForCloud(session) },
  )
}

export function deleteCloudPracticeProgress(
  bankKey: string, expectedRevision: number,
): Promise<CloudProgressSummary> {
  return request<CloudProgressSummary>(
    'api/study/progress/' + encodeURIComponent(bankKey), 'DELETE',
    { expectedRevision, deviceId: getDeviceId() },
  )
}
