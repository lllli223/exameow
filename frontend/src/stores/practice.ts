import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import type { QuestionBank, PracticeSession, PracticeSessionItem, PracticeMode, MockExamConfig, Question, PracticeFilter } from '@exameow/shared'
import {
  recordStudyAttempt,
  recordStudySessionFinish,
  isStudySyncConfigured,
  listRemoteStudyBanks,
  getRemoteStudyBank,
  type StudyBankEnvelope,
} from '@/services/studySync'
import { analyzeCSV, analyzeExcel, parseWithMapping } from '@/utils/importParser'
import type { ColumnMapping, ImportAnalysis } from '@/utils/importParser'
import { usePracticeHistoryStore } from '@/stores/practiceHistory'
import { useWrongQuestionsStore } from '@/stores/wrongQuestions'
import { matchPracticeFilter, reconcileMockConfig } from '@/utils/practiceFilter'
import { gradeTrueFalseAnswer } from '@/utils/answerGrading'
import { createPracticeProgressCoordinator } from '@/services/practiceProgressCoordinator'
import { createPersonalBankCoordinator } from '@/services/personalBankCoordinator'
import { isPersonalBank } from '@/services/personalBankSync'
import { backupPracticeProgress, fingerprintSession, loadCloudProgressMeta } from '@/services/practiceProgressSync'
import {
  loadSavedPracticeSessions,
  saveStoredPracticeSession,
  removeStoredPracticeSession,
  getActivePracticeBankId,
  setActivePracticeBankId,
} from '@/utils/practiceSessionStorage'

const STORAGE_KEY = 'exameow-banks'

function loadBanks(): QuestionBank[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return raw ? JSON.parse(raw) : []
  } catch {
    return []
  }
}

function saveBanks(banks: QuestionBank[]): boolean {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(banks))
    return true
  } catch (error) {
    console.warn('[practice] Could not persist question banks:', error)
    return false
  }
}

function generateId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return crypto.randomUUID()
  return Date.now().toString(36) + '-' + Math.random().toString(36).slice(2, 8)
}

function stopItemTimer(item: PracticeSessionItem | undefined, now = Date.now()) {
  if (!item || item.submitted || item.viewStartedAt == null) return
  item.durationMs = Math.max(0, item.durationMs ?? 0) + Math.max(0, now - item.viewStartedAt)
  item.viewStartedAt = undefined
}

function startItemTimer(item: PracticeSessionItem | undefined, now = Date.now()) {
  if (!item || item.submitted || item.viewStartedAt != null) return
  item.viewStartedAt = now
}

function shuffleArray<T>(arr: T[]): T[] {
  const a = [...arr]
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [a[i]!, a[j]!] = [a[j]!, a[i]!]
  }
  return a
}

function shuffleOptions(questions: Question[]): Question[] {
  return questions.map(q => {
    if (q.type === 'single_choice' || q.type === 'multi_choice') {
      const indices = q.options.map((_, i) => i)
      const shuffled = shuffleArray(indices)
      const newOptions = shuffled.map(i => q.options[i] ?? '')
      const answerMap: Record<string, number> = {}
      q.options.forEach((opt, i) => { answerMap[String.fromCharCode(65 + i)] = i })
      const newAnswer = q.answer
        .toUpperCase()
        .split('')
        .filter(ch => /[A-H]/.test(ch))
        .map(ch => {
          const oldIdx = answerMap[ch]
          if (oldIdx !== undefined) {
            const newIdx = shuffled.indexOf(oldIdx)
            return newIdx >= 0 ? String.fromCharCode(65 + newIdx) : ch
          }
          return ch
        })
        .sort()
        .join('')
      return { ...q, options: newOptions, answer: newAnswer }
    }
    return q
  })
}

function applyPracticeFilter(questions: Question[], filter?: PracticeFilter): Question[] {
  if (!filter) return questions
  const subjects = filter.subjects?.filter(Boolean)
  const chapters = filter.chapters?.filter(Boolean)
  const difficulties = filter.difficulties?.filter(Boolean)
  const types = filter.types?.filter(Boolean)
  if ((filter.difficulties !== undefined && difficulties?.length === 0)
    || (filter.types !== undefined && types?.length === 0)) return []
  if (!subjects?.length && !chapters?.length && !filter.includeUnchaptered && !difficulties?.length && !types?.length) return questions
  return questions.filter(q => matchPracticeFilter(q, filter))
}

function generateMockQuestions(bank: QuestionBank, config: MockExamConfig): Question[] {
  const selected: Question[] = []
  for (const [qtype, count] of Object.entries(config.typeCounts)) {
    if (count <= 0) continue
    const pool = bank.questions.filter(q => q.type === qtype)
    const shuffled = shuffleArray(pool)
    selected.push(...shuffled.slice(0, count))
  }
  return shuffleArray(selected)
}

export interface StudyBankSyncResult {
  total: number
  added: number
  updated: number
  unchanged: number
}

function remoteBankToLocal(envelope: StudyBankEnvelope): QuestionBank {
  const remote = envelope.bank
  const questions = remote.questions.map((q, index) => {
    const originalId = q.id || q.stableKey || `question-${index + 1}`
    return {
      ...q,
      id: originalId,
      stableKey: q.stableKey,
      options: q.options ?? [],
      subject: q.subject ?? remote.subject ?? remote.metadata.subject,
      chapter: q.chapter ?? remote.chapter,
      tags: q.tags ?? remote.tags,
      sourceMeta: {
        ...(remote.sourceMeta ?? {}),
        ...(q.sourceMeta ?? {}),
        remoteBankKey: envelope.bankKey,
        remoteBankVersion: remote.version,
        remoteExam: remote.metadata.exam,
        remoteOutlineVersion: remote.metadata.outlineVersion,
      },
    }
  })
  return {
    id: `study-bank:${envelope.bankKey}`,
    name: remote.name || envelope.name,
    questions,
    createdAt: envelope.createdAt || envelope.updatedAt || Date.now(),
    source: 'server-sync',
    remoteKey: envelope.bankKey,
    remoteUpdatedAt: envelope.updatedAt,
    remoteContentHash: envelope.contentHash,
  }
}

export const usePracticeStore = defineStore('practice', () => {
  const banks = ref<QuestionBank[]>(loadBanks())
  const savedSessions = ref<Record<string, PracticeSession>>(loadSavedPracticeSessions())
  const activeBankId = getActivePracticeBankId()
  const lastSession = activeBankId ? savedSessions.value[activeBankId] : undefined
  const mostRecentUnfinished = Object.values(savedSessions.value)
    .filter(s => s.finishedAt == null)
    .sort((a, b) => b.startedAt - a.startedAt)[0]
  const session = ref<PracticeSession | null>(
    lastSession?.finishedAt == null ? (lastSession ?? mostRecentUnfinished ?? null) : (mostRecentUnfinished ?? null),
  )
  const sessionSaveError = ref(false)
  const bankSaveError = ref(false)
  const progressSync = createPracticeProgressCoordinator({
    getBanks: () => banks.value,
    getSession: bankId => savedSessions.value[bankId] ?? null,
    applyRemote: (bankId, remote) => applyRemoteProgress(bankId, remote),
  })
  const personalBankSync = createPersonalBankCoordinator({
    list: () => banks.value,
    get: bankId => banks.value.find(bank => bank.id === bankId),
    apply: (bankId, remote, explicit) => applyPersonalBank(bankId, remote, explicit),
  })
  const importing = ref(false)
  const importPreview = ref<Question[] | null>(null)
  const importFileName = ref('')
  const importAnalysis = ref<ImportAnalysis | null>(null)
  const importSource = ref('csv')

  const hasSession = computed(() => session.value !== null && session.value.finishedAt == null && session.value.mode !== 'wrong')
  const currentQuestion = computed(() => {
    if (!session.value) return null
    return session.value.questions[session.value.currentIndex] ?? null
  })
  const progress = computed(() => {
    if (!session.value) return { current: 0, total: 0 }
    return { current: session.value.currentIndex + 1, total: session.value.questions.length }
  })
  const isLastQuestion = computed(() => {
    if (!session.value) return false
    return session.value.currentIndex >= session.value.questions.length - 1
  })
  const isFirstQuestion = computed(() => {
    if (!session.value) return false
    return session.value.currentIndex === 0
  })
  const answeredCount = computed(() => {
    if (!session.value) return 0
    return session.value.questions.filter(q => q.userAnswer !== null).length
  })
  const hasUnanswered = computed(() => {
    if (!session.value) return false
    return session.value.questions.some(q => q.userAnswer === null)
  })
  const score = computed(() => {
    if (!session.value) return 0
    const autoGraded = session.value.questions.filter(q => q.isCorrect !== null && q.isCorrect)
    return autoGraded.length
  })
  const autoGradedCount = computed(() => {
    if (!session.value) return 0
    return session.value.questions.filter(q => q.isCorrect !== null).length
  })

  /** Persist only this bank's session, never overwriting another bank's progress. */
  function saveSession(current: PracticeSession | null) {
    if (!current || current.mode === 'wrong') return
    savedSessions.value[current.bankId] = current
    sessionSaveError.value = !saveStoredPracticeSession(current)
    setActivePracticeBankId(current.bankId)
    progressSync.localChanged(current.bankId)
  }

  function getBankSession(bankId: string): PracticeSession | null {
    return savedSessions.value[bankId] ?? null
  }

  function pauseCurrentTimer() {
    const current = session.value
    if (!current || current.finishedAt != null) return
    stopItemTimer(current.questions[current.currentIndex])
    saveSession(current)
  }

  function activateBankSession(bankId: string): boolean {
    const saved = getBankSession(bankId)
    if (!saved) return false
    if (session.value !== saved) pauseCurrentTimer()
    session.value = saved
    setActivePracticeBankId(bankId)
    return true
  }

  function discardTransientSession() {
    if (session.value?.mode === 'wrong') session.value = null
  }

  function forgetSavedSession(bankId: string) {
    delete savedSessions.value[bankId]
    removeStoredPracticeSession(bankId)
    if (getActivePracticeBankId() === bankId) setActivePracticeBankId(null)
  }

  function applyRemoteProgress(bankId: string, remote: PracticeSession | null): boolean {
    const bank = getBank(bankId)
    if (!bank) return false
    if (!remote) {
      forgetSavedSession(bankId)
      if (session.value?.bankId === bankId && session.value.mode !== 'wrong') session.value = null
      return true
    }
    // The stable remote key may have a different device-local bank ID.
    const normalized: PracticeSession = {
      ...remote,
      bankId,
      bankKey: bank.remoteKey ?? bank.id,
      questions: remote.questions.map(item => ({ ...item, viewStartedAt: undefined })),
    }
    if (!saveStoredPracticeSession(normalized)) {
      sessionSaveError.value = true
      return false
    }
    savedSessions.value[bankId] = normalized
    if (session.value?.bankId === bankId && session.value.mode !== 'wrong') session.value = normalized
    sessionSaveError.value = false
    return true
  }

  function applyPersonalBank(bankId: string, remote: QuestionBank | null, explicit = false): boolean {
    const existing = getBank(bankId)
    if (existing && !isPersonalBank(existing)) return false
    if (remote && (!isPersonalBank(remote) || remote.id !== bankId)) return false
    const previousSession = savedSessions.value[bankId]
    if (!remote && !explicit && session.value?.bankId === bankId
        && session.value.mode !== 'wrong' && session.value.finishedAt == null) {
      // Do not kick someone out of an active practice because another device
      // deleted the bank while this page was open.
      return false
    }
    if (!remote && previousSession && !explicit) {
      // A device with unsynced practice data must not lose its bank because
      // another device removed it. Show a conflict and require a choice.
      const acknowledged = loadCloudProgressMeta()[bankId]?.fingerprint
      if (!acknowledged || fingerprintSession(previousSession) !== acknowledged) return false
    }
    const next = banks.value.filter(bank => bank.id !== bankId)
    if (remote) next.push(remote)
    // Do not erase the only local in-progress session if backup storage fails.
    const priorSession = !remote ? savedSessions.value[bankId] : undefined
    if (priorSession && !backupPracticeProgress(bankId, 'before-cloud-bank-deletion', priorSession)) {
      bankSaveError.value = true
      return false
    }
    if (!saveBanks(next)) {
      bankSaveError.value = true
      return false
    }
    if (!remote) {
      forgetSavedSession(bankId)
      if (session.value?.bankId === bankId && session.value.mode !== 'wrong') session.value = null
    }
    banks.value = next
    bankSaveError.value = false
    return true
  }

  function addBank(bank: QuestionBank): boolean {
    const next = [...banks.value, bank]
    if (!saveBanks(next)) {
      bankSaveError.value = true
      return false
    }
    banks.value = next
    bankSaveError.value = false
    if (isPersonalBank(bank)) personalBankSync.localChanged(bank.id)
    return true
  }

  function removeBank(id: string) {
    const existing = getBank(id)
    if (!existing) return
    const next = banks.value.filter(bank => bank.id !== id)
    if (!saveBanks(next)) {
      bankSaveError.value = true
      return
    }
    banks.value = next
    bankSaveError.value = false
    if (isPersonalBank(existing)) personalBankSync.localDeleted(id)
    forgetSavedSession(id)
    if (session.value?.bankId === id) session.value = null
  }

  function getBank(id: string): QuestionBank | undefined {
    return banks.value.find(b => b.id === id)
  }

  function saveGeneratedAsBank(questions: Question[], sourceName: string) {
    if (questions.length === 0) return
    const today = new Date()
    const dateStr = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, '0')}-${String(today.getDate()).padStart(2, '0')}`
    const bank: QuestionBank = {
      id: generateId(),
      name: `AI 出题 - ${dateStr} (${sourceName || '题库'})`,
      questions,
      createdAt: today.getTime(),
      source: 'ai-generated',
    }
    addBank(bank)
  }

  function startSession(bankId: string, mode: PracticeMode, mockConfig?: MockExamConfig, customQuestions?: Question[], filter?: PracticeFilter): boolean {
    const bank = getBank(bankId)
    if (!bank) return false

    const baseQuestions = customQuestions ?? applyPracticeFilter(bank.questions, filter)
    const normalizedMockConfig = mockConfig
      ? reconcileMockConfig(mockConfig, baseQuestions)
      : undefined
    let questions: Question[]
    if (customQuestions) {
      questions = customQuestions
    } else if (mode === 'mock' && normalizedMockConfig) {
      questions = generateMockQuestions({ ...bank, questions: baseQuestions }, normalizedMockConfig)
    } else if (mode === 'sequential') {
      questions = [...baseQuestions]
    } else {
      questions = shuffleArray([...baseQuestions])
    }

    if (mode === 'random') {
      questions = shuffleOptions(questions)
    }

    if (questions.length === 0) return false

    pauseCurrentTimer()
    const startedAt = Date.now()
    const reviewStore = useWrongQuestionsStore()
    const sessionQuestions: PracticeSessionItem[] = questions.map((q, i) => ({
      question: { ...q, id: `${q.id}-s${i}` },
      userAnswer: null as string | null,
      isCorrect: null as boolean | null,
      submitted: false,
      flagged: reviewStore.getWrongEntry(bankId, q.id)?.flagged === true,
      attemptId: generateId(),
      durationMs: 0,
      viewStartedAt: i === 0 ? startedAt : undefined,
    }))

    session.value = {
      bankId,
      bankKey: bank.remoteKey ?? bank.id,
      mode,
      questions: sessionQuestions,
      currentIndex: 0,
      startedAt,
      finishedAt: null,
      mockConfig: mode === 'mock' ? normalizedMockConfig : undefined,
      filter: mode === 'wrong' ? undefined : filter,
      sessionKey: generateId(),
    }
    saveSession(session.value)
    return true
  }

  const currentSubmitted = computed(() => {
    if (!session.value) return false
    const item = session.value.questions[session.value.currentIndex]
    return item ? item.submitted === true : false
  })

  function resumeCurrentTimer() {
    const s = session.value
    if (!s || s.finishedAt != null) return
    startItemTimer(s.questions[s.currentIndex])
    saveSession(s)
  }

  function setAnswer(answer: string | null) {
    if (!session.value) return
    const item = session.value.questions[session.value.currentIndex]
    if (!item) return
    item.userAnswer = answer
    saveSession(session.value)
  }

  function submitAnswer(answer: string | null): boolean | null {
    const s = session.value
    if (!s) return null
    const item = s.questions[s.currentIndex]
    if (!item) return null
    item.userAnswer = answer
    const submittedAt = Date.now()
    stopItemTimer(item, submittedAt)
    item.submitted = true
    if (!item.submittedAt) item.submittedAt = submittedAt

    const q = item.question
    if (q.type === 'single_choice' || q.type === 'multi_choice') {
      const userAns = (answer ?? '').trim().toUpperCase().replace(/[^A-H]/g, '').split('').sort().join('')
      const correctAns = q.answer.trim().toUpperCase().replace(/[^A-H]/g, '').split('').sort().join('')
      item.isCorrect = userAns === correctAns
    } else if (q.type === 'true_false') {
      item.isCorrect = gradeTrueFalseAnswer(answer, q.answer)
    } else if (q.type === 'fill_blank') {
      const userAns = (answer ?? '').trim().toLowerCase()
      const correctAns = q.answer.trim().toLowerCase()
      item.isCorrect = userAns !== '' && userAns === correctAns
    }

    usePracticeHistoryStore().record(q.type, item.isCorrect)
    ensureSyncMeta()
    recordStudyAttempt(s, item)
    saveSession(s)
    return item.isCorrect
  }

  function selfCheck(isCorrect: boolean) {
    const s = session.value
    if (!s) return
    const item = s.questions[s.currentIndex]
    if (!item) return
    item.isCorrect = isCorrect
    const submittedAt = Date.now()
    stopItemTimer(item, submittedAt)
    item.submitted = true
    if (!item.submittedAt) item.submittedAt = submittedAt
    usePracticeHistoryStore().record(item.question.type, isCorrect)
    ensureSyncMeta()
    recordStudyAttempt(s, item)
    saveSession(s)
  }

  /** Lazily backfill stable sync identifiers on sessions created before study sync existed */
  function ensureSyncMeta() {
    const s = session.value
    if (!s) return
    const bank = getBank(s.bankId)
    if (!s.sessionKey) s.sessionKey = generateId()
    if (!s.bankKey) s.bankKey = bank?.remoteKey ?? s.bankId
    for (const item of s.questions) {
      if (!item.attemptId) item.attemptId = generateId()
      if (item.flagged == null) item.flagged = false
      if (!item.question.stableKey && bank) {
        const originalId = item.question.id.replace(/-s\d+$/, '')
        const original = bank.questions.find(q => q.id === originalId)
        if (original?.stableKey) item.question.stableKey = original.stableKey
      }
    }
  }

  const currentFlagged = computed(() => {
    if (!session.value) return false
    const item = session.value.questions[session.value.currentIndex]
    return item?.flagged === true
  })

  /**
   * Toggle the "不确定/需复习" flag on the current question. May be used before
   * or after submit; when already submitted the same attempt (same
   * idempotencyKey) is re-enqueued so the server updates the existing row.
   */
  function toggleFlagCurrent() {
    const s = session.value
    if (!s) return
    const item = s.questions[s.currentIndex]
    if (!item) return
    ensureSyncMeta()
    item.flagged = !item.flagged
    const originalId = item.question.id.replace(/-s\d+$/, '')
    useWrongQuestionsStore().setFlagged(s.bankId, originalId, item.flagged)
    saveSession(s)
    if (item.submitted) {
      recordStudyAttempt(s, item)
    }
  }

  /** Explicit removal from the review book also clears stale session flags. */
  function clearSessionReviewFlag(bankId: string, questionId?: string) {
    const s = session.value
    if (!s || s.bankId !== bankId) return
    let changed = false
    for (const item of s.questions) {
      const originalId = item.question.id.replace(/-s\d+$/, '')
      if (!item.flagged || (questionId && originalId !== questionId)) continue
      item.flagged = false
      changed = true
      if (item.submitted) {
        ensureSyncMeta()
        recordStudyAttempt(s, item)
      }
    }
    if (changed) saveSession(s)
  }

  function saveAiAnalysis(questionId: string, text: string) {
    const originalId = questionId.replace(/-s\d+$/, '')
    const bank = session.value ? getBank(session.value.bankId) : undefined
    const original = bank?.questions.find(q => q.id === originalId)
    if (original) {
      original.aiAnalysis = text
      saveBanks(banks.value)
    }
    if (session.value) {
      for (const item of session.value.questions) {
        if (item.question.id.replace(/-s\d+$/, '') === originalId) {
          item.question.aiAnalysis = text
        }
      }
      saveSession(session.value)
    }
  }

  function nextQuestion() {
    if (!session.value) return
    if (session.value.currentIndex < session.value.questions.length - 1) {
      const now = Date.now()
      stopItemTimer(session.value.questions[session.value.currentIndex], now)
      session.value.currentIndex++
      startItemTimer(session.value.questions[session.value.currentIndex], now)
      saveSession(session.value)
    }
  }

  function prevQuestion() {
    if (!session.value) return
    if (session.value.currentIndex > 0) {
      const now = Date.now()
      stopItemTimer(session.value.questions[session.value.currentIndex], now)
      session.value.currentIndex--
      startItemTimer(session.value.questions[session.value.currentIndex], now)
      saveSession(session.value)
    }
  }

  function goToQuestion(index: number) {
    if (!session.value) return
    if (index >= 0 && index < session.value.questions.length && index !== session.value.currentIndex) {
      const now = Date.now()
      stopItemTimer(session.value.questions[session.value.currentIndex], now)
      session.value.currentIndex = index
      startItemTimer(session.value.questions[session.value.currentIndex], now)
      saveSession(session.value)
    }
  }

  function finishSession() {
    if (!session.value) return
    ensureSyncMeta()
    const finishedAt = Date.now()
    stopItemTimer(session.value.questions[session.value.currentIndex], finishedAt)
    session.value.finishedAt = finishedAt
    saveSession(session.value)
    recordStudySessionFinish(session.value)
  }

  function removeCurrentQuestion() {
    if (!session.value) return
    const idx = session.value.currentIndex
    const now = Date.now()
    stopItemTimer(session.value.questions[idx], now)
    session.value.questions.splice(idx, 1)
    if (session.value.questions.length === 0) {
      ensureSyncMeta()
      session.value.finishedAt = Date.now()
      saveSession(session.value)
      recordStudySessionFinish(session.value)
      return true
    }
    if (idx >= session.value.questions.length) {
      session.value.currentIndex = session.value.questions.length - 1
    }
    startItemTimer(session.value.questions[session.value.currentIndex], now)
    saveSession(session.value)
    return false
  }

  function clearSession() {
    const current = session.value
    if (current && current.mode !== 'wrong') {
      forgetSavedSession(current.bankId)
      progressSync.localDeleted(current.bankId)
    }
    session.value = null
  }

  function getElapsedTime(): number {
    if (!session.value) return 0
    const end = session.value.finishedAt ?? Date.now()
    return Math.floor((end - session.value.startedAt) / 1000)
  }

  function formatTime(seconds: number): string {
    const m = Math.floor(seconds / 60)
    const s = seconds % 60
    if (m > 0) {
      return `${m}m ${s}s`
    }
    return `${s}s`
  }

  function handleAnalysis(analysis: ImportAnalysis | null, fileName: string, source: string): number {
    importFileName.value = fileName
    importSource.value = source
    if (!analysis) {
      importAnalysis.value = null
      importPreview.value = []
      return 0
    }
    if (analysis.missing.length > 0) {
      importAnalysis.value = analysis
      importPreview.value = null
      return 0
    }
    importAnalysis.value = null
    const questions = parseWithMapping(analysis, analysis.mapping, source)
    importPreview.value = questions
    return questions.length
  }

  async function importCSV(text: string, fileName: string): Promise<number> {
    importing.value = true
    try {
      return handleAnalysis(analyzeCSV(text), fileName, 'csv')
    } finally {
      importing.value = false
    }
  }

  async function importExcelFile(buffer: ArrayBuffer, fileName: string): Promise<number> {
    importing.value = true
    try {
      const source = fileName.toLowerCase().endsWith('.xlsx') ? 'xlsx' : 'excel'
      return handleAnalysis(analyzeExcel(buffer), fileName, source)
    } finally {
      importing.value = false
    }
  }

  function applyImportMapping(mapping: ColumnMapping): number {
    if (!importAnalysis.value) return 0
    const questions = parseWithMapping(importAnalysis.value, mapping, importSource.value)
    importPreview.value = questions
    importAnalysis.value = null
    return questions.length
  }

  function confirmImport(): string {
    if (!importPreview.value || importPreview.value.length === 0) return ''
    const source = importSource.value === 'csv' ? 'csv-import' as const : 'xlsx-import' as const
    const nameBase = importFileName.value.replace(/\.[^/.]+$/, '')
    const bank: QuestionBank = {
      id: generateId(),
      name: nameBase || `Imported bank ${new Date().toLocaleDateString()}`,
      questions: [...importPreview.value],
      createdAt: Date.now(),
      source,
    }
    // Avoid repeated imports of the same file within this device, including
    // copies already downloaded from another device.
    const duplicate = banks.value.find(existing =>
      existing.name === bank.name
      && existing.questions.length === bank.questions.length
      && JSON.stringify(existing.questions) === JSON.stringify(bank.questions),
    )
    const id = duplicate?.id ?? (addBank(bank) ? bank.id : '')
    if (!id) return ''
    importPreview.value = null
    importFileName.value = ''
    importAnalysis.value = null
    return id
  }

  function cancelImport() {
    importPreview.value = null
    importFileName.value = ''
    importAnalysis.value = null
  }

  let studyBankSyncInFlight: Promise<StudyBankSyncResult> | null = null

  async function syncStudyBanks(): Promise<StudyBankSyncResult> {
    if (studyBankSyncInFlight) return studyBankSyncInFlight
    const task = syncAllStudyBanks()
    studyBankSyncInFlight = task
    try { return await task }
    finally { if (studyBankSyncInFlight === task) studyBankSyncInFlight = null }
  }

  async function syncAllStudyBanks(): Promise<StudyBankSyncResult> {
    const summaries = await listRemoteStudyBanks()
    const snapshot = [...banks.value]
    const downloaded: QuestionBank[] = []
    let added = 0
    let updated = 0
    let unchanged = 0

    for (const summary of summaries) {
      const existing = snapshot.find(
        bank => bank.source === 'server-sync' && bank.remoteKey === summary.bankKey,
      )
      const sameHash = existing?.remoteContentHash && summary.contentHash
        ? existing.remoteContentHash === summary.contentHash
        : false
      if (existing && (sameHash || existing.remoteUpdatedAt === summary.updatedAt)) {
        unchanged++
        continue
      }

      const envelope = await getRemoteStudyBank(summary.bankKey)
      downloaded.push(remoteBankToLocal(envelope))
      if (existing) updated++
      else added++
    }

    // Commit only after every required download succeeded. Merge into the
    // latest local list so a user import created while network I/O was in
    // flight cannot be overwritten by an older snapshot.
    if (downloaded.length > 0) {
      const merged = [...banks.value]
      for (const localBank of downloaded) {
        const index = merged.findIndex(
          bank => bank.source === 'server-sync' && bank.remoteKey === localBank.remoteKey,
        )
        if (index >= 0) merged[index] = localBank
        else merged.push(localBank)
      }
      if (!saveBanks(merged)) {
        bankSaveError.value = true
        throw new Error('题库下载完成，但本地存储空间不足，无法安全保存。')
      }
      banks.value = merged
      bankSaveError.value = false
    }
    // Personal banks must be available before the full practice sessions are restored.
    await personalBankSync.sync()
    await progressSync.sync()
    return { total: summaries.length, added, updated, unchanged }
  }

  // One ordered reconnect handler: first download personal-bank content, then
  // restore its progress. Separate concurrent focus listeners could miss a
  // newly created bank on the first reconnect.
  if (typeof window !== 'undefined') {
    const reconnect = () => {
      if (isStudySyncConfigured()) void syncStudyBanks().catch(error => {
        console.warn('[practice] Automatic bank/progress sync failed:', error)
      })
    }
    window.addEventListener('online', reconnect)
    window.addEventListener('focus', reconnect)
  }

  return {
    banks,
    session,
    savedSessions,
    sessionSaveError,
    bankSaveError,
    personalBankConflicts: personalBankSync.conflicts,
    personalBankError: personalBankSync.error,
    personalBankSyncing: personalBankSync.syncing,
    syncPersonalBanks: personalBankSync.sync,
    flushPersonalBanks: personalBankSync.flush,
    resolvePersonalBank: personalBankSync.resolve,
    cloudProgressConflicts: progressSync.conflicts,
    cloudProgressSyncing: progressSync.syncing,
    cloudProgressError: progressSync.error,
    syncPracticeProgress: progressSync.sync,
    flushPracticeProgress: progressSync.flush,
    resolveCloudProgress: progressSync.resolve,
    getBankSession,
    activateBankSession,
    pauseCurrentTimer,
    discardTransientSession,
    importing,
    importPreview,
    importFileName,
    hasSession,
    currentQuestion,
    progress,
    isLastQuestion,
    isFirstQuestion,
    answeredCount,
    hasUnanswered,
    currentSubmitted,
    currentFlagged,
    score,
    autoGradedCount,
    addBank,
    removeBank,
    getBank,
    saveGeneratedAsBank,
    startSession,
    resumeCurrentTimer,
    setAnswer,
    submitAnswer,
    selfCheck,
    toggleFlagCurrent,
    clearSessionReviewFlag,
    saveAiAnalysis,
    nextQuestion,
    prevQuestion,
    goToQuestion,
    finishSession,
    removeCurrentQuestion,
    clearSession,
    getElapsedTime,
    formatTime,
    importCSV,
    importExcelFile,
    applyImportMapping,
    importAnalysis,
    confirmImport,
    cancelImport,
    syncStudyBanks,
  }
})
