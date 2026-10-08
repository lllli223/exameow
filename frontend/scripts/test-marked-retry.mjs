/** Run with: node frontend/scripts/test-marked-retry.mjs (no browser/network). */
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import { createServer } from 'vite'
import { createPinia, setActivePinia } from 'pinia'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const storage = new Map()
globalThis.localStorage = {
  getItem: (key) => storage.get(key) ?? null,
  setItem: (key, value) => storage.set(key, String(value)),
  removeItem: (key) => storage.delete(key),
}

const server = await createServer({
  root,
  appType: 'custom',
  logLevel: 'error',
  server: { middlewareMode: true, hmr: false },
})

try {
  const { usePracticeStore } = await server.ssrLoadModule('/src/stores/practice.ts')
  const { useWrongQuestionsStore } = await server.ssrLoadModule('/src/stores/wrongQuestions.ts')
  setActivePinia(createPinia())
  const practice = usePracticeStore()
  const review = useWrongQuestionsStore()
  const bankId = 'review-test-bank'
  const questions = [
    { id: 'q1', stem: 'Correct but unsure', type: 'single_choice', options: ['A', 'B'], answer: 'A', analysis: '' },
    { id: 'q2', stem: 'Wrong and unsure', type: 'single_choice', options: ['A', 'B'], answer: 'A', analysis: '' },
    { id: 'q3', stem: 'Wrong only', type: 'single_choice', options: ['A', 'B'], answer: 'A', analysis: '' },
  ]
  practice.addBank({ id: bankId, name: 'Review regression bank', createdAt: 1, source: 'csv-import', questions })

  // A correct question manually marked before answer appears in wrong retry.
  assert.equal(practice.startSession(bankId, 'sequential'), true)
  practice.toggleFlagCurrent()
  assert.equal(practice.currentFlagged, true)
  assert.equal(review.hasWrongQuestions(bankId), true)
  assert.equal(review.getWrongCount(bankId), 1)
  assert.deepEqual(review.getWrongQuestions(bankId, 'count-desc').map(q => q.id), ['q1'])
  assert.equal(review.getWrongEntry(bankId, 'q1')?.wrongCount, 0)
  assert.equal(practice.submitAnswer('A'), true)
  review.recordCorrect(bankId, 'q1')
  assert.equal(review.getWrongCount(bankId), 1, 'correct answers should not remove manual marks')

  // Wrong and manually marked are one question, not duplicates.
  practice.goToQuestion(1)
  practice.toggleFlagCurrent()
  assert.equal(practice.submitAnswer('B'), false)
  review.recordWrong(bankId, 'q2')
  assert.equal(review.getWrongCount(bankId), 2)
  assert.deepEqual(review.getWrongQuestions(bankId, 'count-desc').map(q => q.id), ['q2', 'q1'])

  // Wrong-only clears after three correct, marked wrong stays reviewable.
  practice.goToQuestion(2)
  assert.equal(practice.submitAnswer('B'), false)
  review.recordWrong(bankId, 'q3')
  for (let i = 0; i < 3; i++) review.recordCorrect(bankId, 'q3')
  assert.equal(review.getWrongEntry(bankId, 'q3'), null)
  for (let i = 0; i < 3; i++) review.recordCorrect(bankId, 'q2')
  assert.equal(review.getWrongEntry(bankId, 'q2')?.wrongCount, 0)
  assert.equal(review.getWrongEntry(bankId, 'q2')?.flagged, true)
  assert.equal(review.getWrongCount(bankId), 2)

  // The retry session restores marked state; unmarking removes marked-only item.
  assert.equal(practice.startSession(bankId, 'wrong', undefined, review.getWrongQuestions(bankId, 'count-desc')), true)
  assert.equal(practice.session?.questions.length, 2)
  assert.equal(practice.currentFlagged, true)
  practice.toggleFlagCurrent()
  assert.equal(review.getWrongCount(bankId), 1)
  assert.equal(review.getWrongEntry(bankId, 'q2'), null)
  assert.deepEqual(review.getWrongQuestions(bankId, 'count-desc').map(q => q.id), ['q1'])

  // Data survives reload; explicitly removing a mark also clears the session flag.
  setActivePinia(createPinia())
  const restoredPractice = usePracticeStore()
  const restoredReview = useWrongQuestionsStore()
  assert.equal(restoredReview.getWrongCount(bankId), 1)
  assert.equal(restoredReview.getWrongEntry(bankId, 'q1')?.flagged, true)
  assert.equal(restoredPractice.startSession(bankId, 'wrong', undefined, restoredReview.getWrongQuestions(bankId, 'count-desc')), true)
  assert.equal(restoredPractice.currentFlagged, true)
  restoredReview.removeWrong(bankId, 'q1')
  assert.equal(restoredPractice.currentFlagged, false)
  assert.equal(restoredReview.getWrongCount(bankId), 0)
  restoredReview.syncSession(restoredPractice.session)
  assert.equal(restoredReview.getWrongCount(bankId), 0)

  // A legacy saved session's flag can be imported without faking a wrong answer.
  restoredReview.syncSession({
    bankId,
    mode: 'sequential',
    questions: [{ question: { ...questions[0], id: 'q1-s0' }, submitted: true, isCorrect: true, flagged: true }],
  })
  assert.equal(restoredReview.getWrongCount(bankId), 1)
  assert.equal(restoredReview.getWrongEntry(bankId, 'q1')?.wrongCount, 0)

  // Recover pre-upgrade marks from the existing read-only server feed once.
  const remoteBankId = 'study-bank:sgcc-os-xuejieyuban-2026'
  restoredPractice.addBank({
    id: remoteBankId,
    remoteKey: 'sgcc-os-xuejieyuban-2026',
    name: 'Legacy server bank',
    createdAt: 2,
    source: 'server-sync',
    questions: [
      { ...questions[0], id: 'server-q1', stableKey: 'stable-1' },
      { ...questions[1], id: 'server-q2', stableKey: 'stable-2' },
    ],
  })
  // Discard outbox records generated by the earlier offline practice test.
  storage.delete('exameow-study-sync-outbox')
  storage.delete('exameow-study-sync-session-outbox')
  storage.set('exameow-study-sync-config', JSON.stringify({ baseUrl: 'https://study.invalid', token: 'private-test-token' }))
  let fetchCount = 0
  globalThis.fetch = async (url, opts) => {
    const requested = new URL(url)
    if (requested.pathname !== '/api/study/feed') return { ok: false, status: 503 }
    fetchCount++
    assert.equal(requested.pathname, '/api/study/feed')
    assert.equal(requested.searchParams.get('bankKey'), 'sgcc-os-xuejieyuban-2026')
    assert.equal(requested.searchParams.get('after'), '0')
    assert.equal(requested.searchParams.get('includeFlagged'), 'true')
    assert.equal(opts.headers.Authorization, 'Bearer private-test-token')
    return {
      ok: true,
      status: 200,
      json: async () => ({
        attempts: [
          { questionKey: 'sgcc-os-xuejieyuban-2026:stable-1', originalQuestionId: 'old-id-1', submittedAt: 1234, flagged: true },
          { questionKey: 'sgcc-os-xuejieyuban-2026:stable-2', originalQuestionId: 'server-q2', submittedAt: 1235, flagged: true },
          { questionKey: 'sgcc-os-xuejieyuban-2026:wrong', originalQuestionId: 'unused', submittedAt: 1236, flagged: false },
        ],
        nextCursor: 3,
      }),
    }
  }
  const recovered = await restoredReview.restoreLegacyServerFlags(remoteBankId)
  assert.equal(recovered, 2)
  assert.equal(restoredReview.getWrongCount(remoteBankId), 2)
  assert.equal(restoredReview.getWrongEntry(remoteBankId, 'server-q1')?.flagged, true)
  assert.equal(restoredReview.getWrongEntry(remoteBankId, 'server-q1')?.wrongCount, 0)
  assert.equal(restoredReview.getWrongEntry(remoteBankId, 'server-q2')?.flagged, true)
  assert.equal(fetchCount, 1)
  assert.equal(await restoredReview.restoreLegacyServerFlags(remoteBankId), 0)
  assert.equal(fetchCount, 1, 'one-time restore must not rescan the review feed')
  restoredReview.setFlagged(remoteBankId, 'server-q1', false)
  assert.equal(await restoredReview.restoreLegacyServerFlags(remoteBankId), 0)
  assert.equal(restoredReview.getWrongCount(remoteBankId), 1, 'unmarking must not re-import historical flags')

  console.log('Marked-uncertain retry integration: PASS (toggle, persistence, dedup, 3-correct, reload, local and server migration)')
} finally {
  await server.close()
}
