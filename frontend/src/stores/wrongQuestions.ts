import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import type { WrongQuestionEntry, WrongSort, Question, PracticeSession } from '@exameow/shared'
import { usePracticeStore } from './practice'
import { listLegacyStudyFlags } from '@/services/studySync'
import { isReviewEntry, recordReviewCorrect, setReviewFlag, sortReviewEntries } from '@/utils/reviewEntries'

const STORAGE_KEY = 'exameow-wrong-questions'

function loadData(): Record<string, Record<string, WrongQuestionEntry>> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return raw ? JSON.parse(raw) : {}
  } catch {
    return {}
  }
}

function saveData(data: Record<string, Record<string, WrongQuestionEntry>>) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(data))
  } catch {}
}

export const useWrongQuestionsStore = defineStore('wrongQuestions', () => {
  const data = ref(loadData())

  function save() {
    saveData(data.value)
  }

  function recordWrong(bankId: string, questionId: string) {
    if (!data.value[bankId]) {
      data.value[bankId] = {}
    }
    const entry = data.value[bankId]![questionId]
    if (entry) {
      entry.wrongCount++
      entry.consecutiveCorrect = 0
      entry.lastWrongAt = Date.now()
    } else {
      data.value[bankId]![questionId] = {
        questionId,
        wrongCount: 1,
        consecutiveCorrect: 0,
        lastWrongAt: Date.now(),
        addedAt: Date.now(),
      }
    }
    save()
  }

  function recordCorrect(bankId: string, questionId: string): boolean {
    const entry = data.value[bankId]?.[questionId]
    if (!entry || entry.wrongCount <= 0) return false
    const result = recordReviewCorrect(entry)
    if (result.entry) {
      data.value[bankId]![questionId] = result.entry
    } else {
      delete data.value[bankId]![questionId]
      if (Object.keys(data.value[bankId]!).length === 0) delete data.value[bankId]
    }
    save()
    return result.removedFromReview
  }

  /** Keep manually marked questions in the retry pool without counting an error. */
  function setFlagged(bankId: string, questionId: string, flagged: boolean, at = Date.now()) {
    const existing = data.value[bankId]?.[questionId]
    if (existing?.flagged === flagged || (!existing && !flagged)) return
    const updated = setReviewFlag(existing, questionId, flagged, at)
    if (updated) {
      if (!data.value[bankId]) data.value[bankId] = {}
      data.value[bankId]![questionId] = updated
    } else if (data.value[bankId]) {
      delete data.value[bankId]![questionId]
      if (Object.keys(data.value[bankId]!).length === 0) delete data.value[bankId]
    }
    save()
  }

  function removeWrong(bankId: string, questionId: string) {
    if (!data.value[bankId]?.[questionId]) return
    delete data.value[bankId]![questionId]
    if (Object.keys(data.value[bankId]!).length === 0) delete data.value[bankId]
    save()
    // Prevent the restored session from re-importing a deliberately removed flag.
    usePracticeStore().clearSessionReviewFlag(bankId, questionId)
  }

  function clearBank(bankId: string) {
    if (!data.value[bankId]) return
    delete data.value[bankId]
    save()
    usePracticeStore().clearSessionReviewFlag(bankId)
  }

  function hasWrongQuestions(bankId: string): boolean {
    const bank = data.value[bankId]
    if (!bank) return false
    return Object.values(bank).some(isReviewEntry)
  }

  function getWrongCount(bankId: string): number {
    const bank = data.value[bankId]
    if (!bank) return 0
    return Object.values(bank).filter(isReviewEntry).length
  }

  function getWrongEntry(bankId: string, questionId: string): WrongQuestionEntry | null {
    return data.value[bankId]?.[questionId] ?? null
  }

  function getBankEntryMap(bankId: string): Record<string, WrongQuestionEntry> {
    return data.value[bankId] ?? {}
  }

  function getWrongQuestions(bankId: string, sort: WrongSort): Question[] {
    const practiceStore = usePracticeStore()
    const bank = practiceStore.getBank(bankId)
    if (!bank) return []

    const entries = data.value[bankId]
    if (!entries) return []

    const sorted = sortReviewEntries(Object.values(entries).filter(isReviewEntry), sort)

    return sorted
      .map(entry => bank.questions.find(q => q.id === entry.questionId))
      .filter((q): q is Question => q !== undefined)
  }

  function syncSession(session: PracticeSession) {
    const bankId = session.bankId
    let changed = false
    for (const item of session.questions) {
      const originalId = item.question.id.replace(/-s\d+$/, '')
      const existing = data.value[bankId]?.[originalId]
      // Repair legacy sessions that were not yet reflected in the local review book.
      if (item.isCorrect === false && (!existing || !isReviewEntry(existing))) {
        if (!data.value[bankId]) data.value[bankId] = {}
        data.value[bankId]![originalId] = {
          questionId: originalId,
          wrongCount: 1,
          consecutiveCorrect: 0,
          lastWrongAt: Date.now(),
          addedAt: Date.now(),
          flagged: existing?.flagged,
          flaggedAt: existing?.flaggedAt,
        }
        changed = true
      }
      // Upgrade a marked-only question from an older saved session once.
      if (item.flagged === true && data.value[bankId]?.[originalId]?.flagged !== true) {
        const updated = setReviewFlag(data.value[bankId]?.[originalId], originalId, true)
        if (updated) {
          if (!data.value[bankId]) data.value[bankId] = {}
          data.value[bankId]![originalId] = updated
          changed = true
        }
      }
    }
    if (changed) save()
  }

  /**
   * Recover pre-upgrade manual marks stored on the self-hosted server.
   * This is an intentionally one-time read-only migration, not a recurring
   * feed consumer: unmarking locally must not re-import old flagged attempts.
   */
  async function restoreLegacyServerFlags(bankId: string): Promise<number> {
    const bank = usePracticeStore().getBank(bankId)
    if (!bank) return 0
    const bankKey = bank.remoteKey ?? bank.id
    const migrationKey = `exameow-legacy-unsure-restored:${encodeURIComponent(bankKey)}`
    if (localStorage.getItem(migrationKey) === '1') return 0

    const oldMarks = await listLegacyStudyFlags(bankKey)
    const byId = new Map(bank.questions.map(q => [q.id, q]))
    const byStableKey = new Map(bank.questions
      .filter(q => q.stableKey)
      .map(q => [`${bankKey}:${q.stableKey}`, q]))
    let restored = 0
    for (const mark of oldMarks) {
      const question = byId.get(mark.originalQuestionId) ?? byStableKey.get(mark.questionKey)
      if (!question || data.value[bankId]?.[question.id]?.flagged === true) continue
      setFlagged(bankId, question.id, true, mark.submittedAt || Date.now())
      restored++
    }
    localStorage.setItem(migrationKey, '1')
    return restored
  }

  function getAllWrongBanks(): { bankId: string; entries: WrongQuestionEntry[] }[] {
    const practiceStore = usePracticeStore()
    return Object.entries(data.value).map(([bankId, entriesMap]) => ({
      bankId,
      entries: Object.values(entriesMap),
    })).filter(item => {
      const bank = practiceStore.getBank(item.bankId)
      return bank !== undefined
    })
  }

  return {
    data,
    recordWrong,
    recordCorrect,
    setFlagged,
    removeWrong,
    clearBank,
    hasWrongQuestions,
    getWrongCount,
    getWrongEntry,
    getBankEntryMap,
    getWrongQuestions,
    getAllWrongBanks,
    syncSession,
    restoreLegacyServerFlags,
  }
})
