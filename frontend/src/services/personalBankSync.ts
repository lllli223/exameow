/**
 * Local-first, revisioned synchronization of user-imported/generated banks.
 *
 * A personal bank's original ID is its permanent cross-device ID. In contrast,
 * operator-managed `study_banks` continue to use the existing read-only bank
 * download endpoint. No device may overwrite a concurrent revision silently.
 */
import type { QuestionBank } from '@exameow/shared'
import { getDeviceId, isStudySyncConfigured, loadStudySyncConfig } from './studySync'
import { ProgressSyncError } from './practiceProgressSync'

export interface PersonalBankSummary {
  bankId: string
  name: string
  revision: number
  questionCount: number
  contentHash: string
  updatedAt: number
  deviceId: string
  deleted: boolean
}
export interface PersonalBankDocument extends PersonalBankSummary {
  bank: QuestionBank | null
}
export interface PersonalBankMeta {
  revision: number
  fingerprint: string
}

const META_KEY = 'exameow-personal-bank-sync-meta'
const TOMBSTONE_KEY = 'exameow-personal-bank-deleted'
const BACKUP_PREFIX = 'exameow-personal-bank-backup:'
const TIMEOUT_MS = 30_000

function read(key: string): string | null {
  try { return localStorage.getItem(key) } catch { return null }
}
function write(key: string, value: string): boolean {
  try {
    localStorage.setItem(key, value)
    return true
  } catch (error) {
    console.warn('[personal-banks] Failed to store sync state:', error)
    return false
  }
}
function readObject<T>(key: string): Record<string, T> {
  try {
    const parsed = JSON.parse(read(key) ?? '{}')
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {}
  } catch { return {} }
}

export function loadPersonalBankMeta(): Record<string, PersonalBankMeta> {
  return readObject<PersonalBankMeta>(META_KEY)
}
export function savePersonalBankMeta(meta: Record<string, PersonalBankMeta>): boolean {
  return write(META_KEY, JSON.stringify(meta))
}
export function loadPersonalBankTombstones(): Record<string, true> {
  return readObject<true>(TOMBSTONE_KEY)
}
export function setPersonalBankTombstone(bankId: string, deleted: boolean): boolean {
  const tombstones = loadPersonalBankTombstones()
  if (deleted) tombstones[bankId] = true
  else delete tombstones[bankId]
  return write(TOMBSTONE_KEY, JSON.stringify(tombstones))
}

export function isPersonalBank(bank: QuestionBank): boolean {
  return bank.source !== 'server-sync'
}

/** Drop transient cloud metadata, but retain every question and its original ID. */
export function personalBankSnapshot(bank: QuestionBank): QuestionBank {
  return {
    id: bank.id,
    name: bank.name,
    source: bank.source,
    createdAt: bank.createdAt,
    questions: bank.questions,
  }
}

/** Deterministic fingerprint, even if a JSON server changes property order. */
function canonical(value: unknown): string {
  if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']'
  if (value !== null && typeof value === 'object') {
    const object = value as Record<string, unknown>
    const props = Object.keys(object).filter(key => object[key] !== undefined).sort()
    return '{' + props.map(key => JSON.stringify(key) + ':' + canonical(object[key])).join(',') + '}'
  }
  return JSON.stringify(value) ?? 'null'
}

export function fingerprintPersonalBank(bank: QuestionBank): string {
  const text = canonical(personalBankSnapshot(bank))
  let a = 2166136261, b = 5381
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i)
    a = Math.imul(a ^ code, 16777619)
    b = Math.imul(b, 33) ^ code
  }
  return text.length + ':' + (a >>> 0).toString(16) + ':' + (b >>> 0).toString(16)
}

export function backupPersonalBank(bankId: string, reason: string, bank: QuestionBank): boolean {
  return write(BACKUP_PREFIX + encodeURIComponent(bankId), JSON.stringify({
    reason, backedUpAt: Date.now(), bank: personalBankSnapshot(bank),
  }))
}

async function request<T>(path: string, method = 'GET', body?: unknown): Promise<T> {
  if (!isStudySyncConfigured()) throw new ProgressSyncError(0, 'not_configured')
  const config = loadStudySyncConfig()
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), TIMEOUT_MS)
  try {
    const response = await fetch(config.baseUrl.replace(/\/+$/, '') + '/' + path, {
      method,
      headers: {
        Authorization: 'Bearer ' + config.token,
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
      signal: controller.signal,
    })
    const result = await response.json().catch(() => ({}))
    if (!response.ok) throw new ProgressSyncError(response.status, result.error || 'http_error')
    return result as T
  } finally { clearTimeout(timeout) }
}

export async function listCloudPersonalBanks(): Promise<PersonalBankSummary[]> {
  const result = await request<{ banks: PersonalBankSummary[] }>('api/study/personal-banks')
  return Array.isArray(result.banks) ? result.banks : []
}

export function getCloudPersonalBank(bankId: string): Promise<PersonalBankDocument> {
  return request<PersonalBankDocument>('api/study/personal-banks/' + encodeURIComponent(bankId))
}
export function putCloudPersonalBank(
  bank: QuestionBank, expectedRevision: number,
): Promise<PersonalBankSummary> {
  return request<PersonalBankSummary>(
    'api/study/personal-banks/' + encodeURIComponent(bank.id), 'PUT', {
      expectedRevision, deviceId: getDeviceId(), bank: personalBankSnapshot(bank),
    },
  )
}
export function deleteCloudPersonalBank(
  bankId: string, expectedRevision: number,
): Promise<PersonalBankSummary> {
  return request<PersonalBankSummary>(
    'api/study/personal-banks/' + encodeURIComponent(bankId), 'DELETE', {
      expectedRevision, deviceId: getDeviceId(),
    },
  )
}
