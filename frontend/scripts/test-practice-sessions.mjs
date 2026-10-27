import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import ts from 'typescript'

// Transpile the production TypeScript utilities (no browser or DOM automation).
// The type-only shared-model import disappears during transpilation.
const transpile = (relative) => ts.transpileModule(
  readFileSync(new URL(relative, import.meta.url), 'utf8'),
  { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } },
).outputText
const dataUrl = (text) => 'data:text/javascript;base64,' + Buffer.from(text).toString('base64')
const gradingUrl = dataUrl(transpile('../src/utils/answerGrading.ts'))
const storageCode = transpile('../src/utils/practiceSessionStorage.ts')
  .replace("from './answerGrading'", "from '" + gradingUrl + "'")
const sourceUrl = dataUrl(storageCode)
const {
  loadSavedPracticeSessions,
  saveStoredPracticeSession,
  removeStoredPracticeSession,
  storedPracticeSessionKey,
  getActivePracticeBankId,
  setActivePracticeBankId,
} = await import(sourceUrl)

class MemoryStorage {
  items = new Map()
  get length() { return this.items.size }
  key(index) { return [...this.items.keys()][index] ?? null }
  getItem(key) { return this.items.get(key) ?? null }
  setItem(key, value) { this.items.set(key, value) }
  removeItem(key) { this.items.delete(key) }
}

function createSession(bankId, index = 0, mode = 'sequential') {
  return {
    bankId, mode, currentIndex: index, startedAt: 1000, finishedAt: null,
    questions: [
      {
        question: { id: 'q1', type: 'true_false', answer: '√' },
        userAnswer: 'A', isCorrect: false, submitted: true,
        flagged: true, durationMs: 2000, viewStartedAt: 1000000,
      },
      {
        question: { id: 'q2', type: 'single_choice', answer: 'B' },
        userAnswer: null, isCorrect: null, submitted: false,
        flagged: false, durationMs: 0,
      },
    ],
  }
}

test('migrates existing single session without losing answers, flags, index or time', () => {
  const store = new MemoryStorage()
  const old = createSession('study-bank:os-chapter-1', 1)
  store.setItem('exameow-practice-session', JSON.stringify(old))
  const all = loadSavedPracticeSessions(store)
  const migrated = all[old.bankId]
  assert.ok(migrated)
  assert.equal(migrated.currentIndex, 1)
  assert.equal(migrated.questions[0].flagged, true)
  assert.equal(migrated.questions[0].durationMs, 2000)
  assert.equal(migrated.questions[0].isCorrect, true) // historical T/F grading repair
  assert.equal(migrated.questions[0].viewStartedAt, undefined) // no offline time
  assert.equal(store.getItem('exameow-practice-session'), null)
  assert.ok(store.getItem(storedPracticeSessionKey(old.bankId)))
})

test('A and B sessions remain independent across reload, switch and deletion', () => {
  const store = new MemoryStorage()
  const a = createSession('study-bank:网络', 1)
  const b = createSession('study-bank:数据结构', 0)
  b.questions[0].userAnswer = 'B'
  b.questions[0].flagged = false
  assert.equal(saveStoredPracticeSession(a, store), true)
  assert.equal(saveStoredPracticeSession(b, store), true)
  setActivePracticeBankId(b.bankId, store)
  assert.equal(getActivePracticeBankId(store), b.bankId)
  const reloaded = loadSavedPracticeSessions(store)
  assert.deepEqual(Object.keys(reloaded).sort(), [a.bankId, b.bankId].sort())
  assert.equal(reloaded[a.bankId].currentIndex, 1)
  assert.equal(reloaded[b.bankId].currentIndex, 0)
  assert.equal(reloaded[a.bankId].questions[0].flagged, true)
  assert.equal(reloaded[b.bankId].questions[0].flagged, false)
  removeStoredPracticeSession(b.bankId, store)
  assert.equal(loadSavedPracticeSessions(store)[a.bankId].currentIndex, 1)
  assert.equal(loadSavedPracticeSessions(store)[b.bankId], undefined)
})

test('wrong-question retry is never persisted as the bank main session', () => {
  const store = new MemoryStorage()
  const main = createSession('study-bank:network', 1)
  saveStoredPracticeSession(main, store)
  const wrong = createSession(main.bankId, 0, 'wrong')
  assert.equal(saveStoredPracticeSession(wrong, store), false)
  assert.equal(loadSavedPracticeSessions(store)[main.bankId].currentIndex, 1)
})

test('corrupt entries cannot erase healthy bank sessions', () => {
  const store = new MemoryStorage()
  saveStoredPracticeSession(createSession('healthy'), store)
  store.setItem('exameow-practice-session:broken', '{not-json')
  store.setItem('exameow-practice-session:other', JSON.stringify(createSession('mismatch')))
  assert.deepEqual(Object.keys(loadSavedPracticeSessions(store)), ['healthy'])
})

test('migration keeps the legacy copy when browser storage is full', () => {
  const store = new MemoryStorage()
  const old = createSession('legacy')
  store.setItem('exameow-practice-session', JSON.stringify(old))
  const origSet = store.setItem.bind(store)
  store.setItem = (key, value) => {
    if (key.startsWith('exameow-practice-session:')) throw new Error('quota')
    origSet(key, value)
  }
  const originalWarn = console.warn
  console.warn = () => {}
  try {
    assert.ok(loadSavedPracticeSessions(store).legacy)
    assert.ok(store.getItem('exameow-practice-session'))
  } finally {
    console.warn = originalWarn
  }
})
