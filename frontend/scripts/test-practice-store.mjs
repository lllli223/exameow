import test from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'vite'
import { createPinia, setActivePinia } from 'pinia'

class MemoryStorage {
  items = new Map()
  get length() { return this.items.size }
  key(index) { return [...this.items.keys()][index] ?? null }
  getItem(key) { return this.items.get(key) ?? null }
  setItem(key, value) { this.items.set(key, value) }
  removeItem(key) { this.items.delete(key) }
}

function bank(id) {
  return {
    id, name: id, createdAt: Date.now(), source: 'server-sync',
    questions: [
      { id: id + ':1', type: 'single_choice', stem: 'Question 1', options: ['A', 'B'], answer: 'A', analysis: '' },
      { id: id + ':2', type: 'single_choice', stem: 'Question 2', options: ['A', 'B'], answer: 'B', analysis: '' },
    ],
  }
}

test('actual Pinia practice store resumes A and B independently through reload, clearing, and wrong review', async () => {
  globalThis.localStorage = new MemoryStorage()
  // Vite SSR evaluates the real store and its aliased imports; no browser automation.
  const vite = await createServer({ server: { middlewareMode: true }, appType: 'custom', logLevel: 'silent' })
  try {
    const { usePracticeStore } = await vite.ssrLoadModule('/src/stores/practice.ts')
    setActivePinia(createPinia())
    const p = usePracticeStore()
    const aId = 'study-bank:network'
    const bId = 'study-bank:ds'
    p.addBank(bank(aId))
    p.addBank(bank(bId))

    assert.equal(p.startSession(aId, 'sequential'), true)
    p.submitAnswer('A')
    p.nextQuestion()
    p.toggleFlagCurrent()
    assert.equal(p.getBankSession(aId).currentIndex, 1)

    assert.equal(p.startSession(bId, 'sequential'), true)
    p.submitAnswer('B')
    p.nextQuestion()
    assert.equal(p.getBankSession(aId).questions[0].isCorrect, true)
    assert.equal(p.getBankSession(bId).questions[0].isCorrect, false)

    // Navigate back to the first bank, then create a fresh Pinia store (page reload).
    assert.equal(p.activateBankSession(aId), true)
    assert.equal(p.currentQuestion.flagged, true)
    assert.equal(p.currentQuestion.question.id.startsWith(aId), true)

    setActivePinia(createPinia())
    const reloaded = usePracticeStore()
    assert.equal(reloaded.getBankSession(aId).currentIndex, 1)
    assert.equal(reloaded.getBankSession(bId).currentIndex, 1)
    assert.equal(reloaded.getBankSession(aId).questions[1].flagged, true)
    assert.equal(reloaded.activateBankSession(bId), true)
    assert.equal(reloaded.currentQuestion.question.id.startsWith(bId), true)

    const bMainKey = localStorage.getItem('exameow-practice-session:' + encodeURIComponent(bId))
    assert.equal(reloaded.startSession(bId, 'wrong', undefined, [bank(bId).questions[0]]), true)
    assert.equal(localStorage.getItem('exameow-practice-session:' + encodeURIComponent(bId)), bMainKey)
    reloaded.discardTransientSession()
    assert.equal(reloaded.activateBankSession(bId), true)
    assert.equal(reloaded.session.mode, 'sequential')

    reloaded.clearSession()
    assert.equal(reloaded.getBankSession(bId), null)
    assert.equal(reloaded.getBankSession(aId).currentIndex, 1)
  } finally {
    await vite.close()
    delete globalThis.localStorage
  }
})
