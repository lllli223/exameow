import type { PracticeSession } from '@exameow/shared'
import { gradeTrueFalseAnswer } from './answerGrading'

const LEGACY_SESSION_KEY = 'exameow-practice-session'
const SESSION_PREFIX = 'exameow-practice-session:'
const ACTIVE_BANK_KEY = 'exameow-practice-active-bank'

/** Each bank has its own durable session; the old single-session key is migrated on first use. */
type SessionStorage = Pick<Storage, 'length' | 'key' | 'getItem' | 'setItem' | 'removeItem'>

function browserStorage(): SessionStorage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

export function storedPracticeSessionKey(bankId: string): string {
  return SESSION_PREFIX + encodeURIComponent(bankId)
}

function parseSession(raw: string | null): PracticeSession | null {
  if (!raw) return null
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!parsed || typeof parsed !== 'object') return null
    const session = parsed as PracticeSession
    if (!session.bankId || typeof session.bankId !== 'string'
      || !['sequential', 'random', 'mock'].includes(session.mode)
      || !Array.isArray(session.questions) || session.questions.length === 0
      || !Number.isInteger(session.currentIndex) || session.currentIndex < 0
      || typeof session.startedAt !== 'number') return null
    if (session.questions.some(item => !item || !item.question || typeof item.question.id !== 'string')) return null

    // The user was not in the practice view while the app was closed.
    session.currentIndex = Math.min(session.currentIndex, session.questions.length - 1)
    for (const item of session.questions) {
      item.viewStartedAt = undefined
      if (item.submitted && item.question.type === 'true_false' && item.isCorrect !== null) {
        item.isCorrect = gradeTrueFalseAnswer(item.userAnswer, item.question.answer)
      }
    }
    return session
  } catch {
    return null
  }
}

export function saveStoredPracticeSession(
  session: PracticeSession,
  storage: SessionStorage | null = browserStorage(),
): boolean {
  if (!storage || session.mode === 'wrong') return false
  try {
    storage.setItem(storedPracticeSessionKey(session.bankId), JSON.stringify(session))
    return true
  } catch (error) {
    console.warn('[practice] Could not persist bank session:', error)
    return false
  }
}

export function removeStoredPracticeSession(bankId: string, storage: SessionStorage | null = browserStorage()) {
  if (!storage) return
  try { storage.removeItem(storedPracticeSessionKey(bankId)) } catch {}
}

export function getActivePracticeBankId(storage: SessionStorage | null = browserStorage()): string | null {
  try { return storage?.getItem(ACTIVE_BANK_KEY) || null } catch { return null }
}

export function setActivePracticeBankId(bankId: string | null, storage: SessionStorage | null = browserStorage()) {
  if (!storage) return
  try {
    if (bankId) storage.setItem(ACTIVE_BANK_KEY, bankId)
    else storage.removeItem(ACTIVE_BANK_KEY)
  } catch {}
}

export function loadSavedPracticeSessions(
  storage: SessionStorage | null = browserStorage(),
): Record<string, PracticeSession> {
  const result: Record<string, PracticeSession> = Object.create(null)
  if (!storage) return result
  try {
    for (let i = 0; i < storage.length; i++) {
      const key = storage.key(i)
      if (!key?.startsWith(SESSION_PREFIX)) continue
      const parsed = parseSession(storage.getItem(key))
      if (parsed && key === storedPracticeSessionKey(parsed.bankId)) {
        result[parsed.bankId] = parsed
      }
    }

    const legacy = parseSession(storage.getItem(LEGACY_SESSION_KEY))
    if (legacy) {
      if (!result[legacy.bankId]) {
        // Only remove the old key after the new key has been saved successfully.
        if (saveStoredPracticeSession(legacy, storage)) {
          result[legacy.bankId] = legacy
          storage.removeItem(LEGACY_SESSION_KEY)
        } else {
          // Keep the session usable this visit and leave the old key for retry.
          result[legacy.bankId] = legacy
        }
      } else {
        // New-format data already exists for this bank and takes precedence.
        storage.removeItem(LEGACY_SESSION_KEY)
      }
    }
  } catch (error) {
    console.warn('[practice] Could not load saved sessions:', error)
  }
  return result
}
