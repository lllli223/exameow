import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import type { QuestionBank, PracticeSession, PracticeSessionItem, PracticeMode, MockExamConfig, Question, PracticeFilter } from '@exameow/shared'
import {
  recordStudyAttempt,
  recordStudySessionFinish,
  listRemoteStudyBanks,
  getRemoteStudyBank,
  type StudyBankEnvelope,
} from '@/services/studySync'
import { analyzeCSV, analyzeExcel, parseWithMapping } from '@/utils/importParser'
import type { ColumnMapping, ImportAnalysis } from '@/utils/importParser'
import { usePracticeHistoryStore } from '@/stores/practiceHistory'
import { matchPracticeFilter, reconcileMockConfig } from '@/utils/practiceFilter'
import { gradeTrueFalseAnswer } from '@/utils/answerGrading'

const STORAGE_KEY = 'exameow-banks'
const SESSION_KEY = 'exameow-practice-session'

function loadBanks(): QuestionBank[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return raw ? JSON.parse(raw) : []
  } catch {
    return []
  }
}

function saveBanks(banks: QuestionBank[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(banks))
  } catch {}
}

function loadSession(): PracticeSession | null {
  try {
    const raw = localStorage.getItem(SESSION_KEY)
    const parsed: PracticeSession | null = raw ? JSON.parse(raw) : null
    if (parsed) {
      let gradesRepaired = false
      for (const item of parsed.questions) {
        if (item.submitted && item.question.type === 'true_false' && item.isCorrect !== null) {
          const repaired = gradeTrueFalseAnswer(item.userAnswer, item.question.answer)
          if (repaired !== item.isCorrect) {
            item.isCorrect = repaired
            gradesRepaired = true
          }
        }
      }
      if (gradesRepaired) localStorage.setItem(SESSION_KEY, JSON.stringify(parsed))

      if (parsed.finishedAt == null) {
        // A restored session is only visible as a resume card initially. Do not
        // count time until the user explicitly resumes the practice view.
        for (const item of parsed.questions) item.viewStartedAt = undefined
      }
    }
    return parsed
  } catch {
    return null
  }
}

function saveSession(session: PracticeSession | null) {
  try {
    if (session && session.mode === 'wrong') return
    if (session) {
      localStorage.setItem(SESSION_KEY, JSON.stringify(session))
    } else {
      localStorage.removeItem(SESSION_KEY)
    }
  } catch {}
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
  const session = ref<PracticeSession | null>(loadSession())
  const importing = ref(false)
  const importPreview = ref<Question[] | null>(null)
  const importFileName = ref('')
  const importAnalysis = ref<ImportAnalysis | null>(null)
  const importSource = ref('csv')

  const hasSession = computed(() => session.value !== null)
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

  function addBank(bank: QuestionBank) {
    banks.value.push(bank)
    saveBanks(banks.value)
  }

  function removeBank(id: string) {
    banks.value = banks.value.filter(b => b.id !== id)
    saveBanks(banks.value)
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

    const startedAt = Date.now()
    const sessionQuestions: PracticeSessionItem[] = questions.map((q, i) => ({
      question: { ...q, id: `${q.id}-s${i}` },
      userAnswer: null as string | null,
      isCorrect: null as boolean | null,
      submitted: false,
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
    saveSession(s)
    if (item.submitted) {
      recordStudyAttempt(s, item)
    }
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
    session.value = null
    saveSession(null)
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
    addBank(bank)
    importPreview.value = null
    importFileName.value = ''
    importAnalysis.value = null
    return bank.id
  }

  function cancelImport() {
    importPreview.value = null
    importFileName.value = ''
    importAnalysis.value = null
  }

  async function syncStudyBanks(): Promise<StudyBankSyncResult> {
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
      banks.value = merged
      saveBanks(banks.value)
    }
    return { total: summaries.length, added, updated, unchanged }
  }

  return {
    banks,
    session,
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
