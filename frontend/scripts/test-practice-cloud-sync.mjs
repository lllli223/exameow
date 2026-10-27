import test from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'vite'
import { createPinia, setActivePinia } from 'pinia'

class MemoryStorage {
  data = new Map()
  get length() { return this.data.size }
  key(i) { return [...this.data.keys()][i] ?? null }
  getItem(k) { return this.data.get(k) ?? null }
  setItem(k, v) { this.data.set(k, v) }
  removeItem(k) { this.data.delete(k) }
}

const makeBank = () => ({
  id: 'study-bank:sample', remoteKey: 'sample', name: 'Sample',
  createdAt: Date.now(), source: 'server-sync',
  questions: ['A', 'B', 'C'].map((answer, i) => ({
    id: 'q' + i, stem: 'Question ' + i, options: ['A', 'B', 'C'],
    type: 'single_choice', answer, analysis: '',
  })),
})

test('cross-device resume, safe conflict resolution and deletion', async () => {
  const originalFetch = globalThis.fetch
  const originalStorage = globalThis.localStorage
  const cloud = new Map()
  let offline = false
  globalThis.fetch = async (input, options = {}) => {
    if (offline) throw new Error('offline')
    const path = new URL(String(input)).pathname
    const method = options.method || 'GET'
    if (path === '/api/study/progress') {
      return Response.json({ progress: [...cloud].map(([bankKey, value]) => ({
        bankKey, revision: value.revision, updatedAt: value.updatedAt,
        deviceId: value.deviceId, deleted: value.session === null,
      })) })
    }
    if (path.startsWith('/api/study/progress/')) {
      const bankKey = decodeURIComponent(path.slice('/api/study/progress/'.length))
      const existing = cloud.get(bankKey)
      if (method === 'GET') {
        return existing
          ? Response.json({ bankKey, ...existing, deleted: existing.session === null })
          : Response.json({ error: 'no_progress' }, { status: 404 })
      }
      const body = JSON.parse(options.body)
      if (body.expectedRevision !== (existing?.revision ?? 0)) {
        return Response.json({ error: 'revision_conflict' }, { status: 409 })
      }
      const updated = {
        revision: (existing?.revision ?? 0) + 1,
        updatedAt: Date.now(), deviceId: body.deviceId,
        session: method === 'DELETE' ? null : structuredClone(body.session),
      }
      cloud.set(bankKey, updated)
      return Response.json({ bankKey, ...updated, deleted: updated.session === null })
    }
    throw new Error('Unexpected URL: ' + path)
  }

  const vite = await createServer({ server: { middlewareMode: true }, appType: 'custom', logLevel: 'silent' })
  try {
    const { usePracticeStore } = await vite.ssrLoadModule('/src/stores/practice.ts')
    const { saveStudySyncConfig } = await vite.ssrLoadModule('/src/services/studySync.ts')
    const storages = [new MemoryStorage(), new MemoryStorage()]
    const createDevice = async index => {
      globalThis.localStorage = storages[index]
      saveStudySyncConfig({ baseUrl: 'https://example.test', token: 'shared-secret' })
      setActivePinia(createPinia())
      const store = usePracticeStore()
      store.addBank(makeBank())
      await store.syncPracticeProgress()
      return store
    }
    const bankId = makeBank().id

    const A = await createDevice(0)
    assert.equal(A.startSession(bankId, 'sequential'), true)
    A.setAnswer('A')
    A.nextQuestion()
    await A.flushPracticeProgress()
    assert.equal(cloud.get('sample').revision, 1)

    const B = await createDevice(1)
    assert.equal(B.getBankSession(bankId).currentIndex, 1)
    assert.equal(B.getBankSession(bankId).questions[0].userAnswer, 'A')
    assert.equal(B.activateBankSession(bankId), true)
    globalThis.localStorage = storages[1]
    B.setAnswer('B')
    B.nextQuestion()
    await B.flushPracticeProgress()
    assert.equal(cloud.get('sample').revision, 2)

    globalThis.localStorage = storages[0]
    await A.syncPracticeProgress()
    assert.equal(A.getBankSession(bankId).currentIndex, 2)
    assert.equal(A.getBankSession(bankId).questions[1].userAnswer, 'B')

    // Both devices make incompatible changes at the same revision.
    offline = true
    A.setAnswer('A')
    await A.flushPracticeProgress()
    offline = false
    globalThis.localStorage = storages[1]
    B.setAnswer('B')
    await B.flushPracticeProgress()
    assert.equal(cloud.get('sample').revision, 3)
    globalThis.localStorage = storages[0]
    await A.syncPracticeProgress()
    assert.ok(A.cloudProgressConflicts[bankId])
    assert.equal(A.getBankSession(bankId).questions[2].userAnswer, 'A')
    assert.equal(cloud.get('sample').session.questions[2].userAnswer, 'B')

    await A.resolveCloudProgress(bankId, 'local')
    assert.equal(A.cloudProgressConflicts[bankId], undefined)
    assert.equal(cloud.get('sample').revision, 4)
    assert.equal(cloud.get('sample').session.questions[2].userAnswer, 'A')

    A.clearSession()
    await A.flushPracticeProgress()
    assert.equal(cloud.get('sample').session, null)
    globalThis.localStorage = storages[1]
    await B.syncPracticeProgress()
    assert.equal(B.getBankSession(bankId), null)
  } finally {
    await vite.close()
    globalThis.fetch = originalFetch
    if (originalStorage === undefined) delete globalThis.localStorage
    else globalThis.localStorage = originalStorage
  }
})
