import test from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'vite'
import { createPinia, setActivePinia } from 'pinia'

class Storage {
  items = new Map()
  get length() { return this.items.size }
  key(index) { return [...this.items.keys()][index] ?? null }
  getItem(key) { return this.items.get(key) ?? null }
  setItem(key, value) { this.items.set(key, value) }
  removeItem(key) { this.items.delete(key) }
}

function bank(id = 'user-original-bank-123') {
  return {
    id, name: '数据结构第一章', createdAt: 1720000000000, source: 'csv-import',
    questions: [
      { id: 'csv-1', type: 'single_choice', stem: '第一题', options: ['A. 正确', 'B. 错误'], answer: 'A', analysis: '解析一' },
      { id: 'csv-2', type: 'true_false', stem: '第二题', options: [], answer: '√', analysis: '解析二' },
      { id: 'csv-3', type: 'single_choice', stem: '第三题', options: ['A', 'B'], answer: 'B', analysis: '解析三' },
    ],
  }
}

test('personal banks sync across devices with original IDs, independent progress, conflicts, offline and deletion', async () => {
  const oldFetch = globalThis.fetch, oldStorage = globalThis.localStorage
  const banks = new Map(), progress = new Map()
  let offline = false
  const summary = (id, record) => ({
    bankId: id, revision: record.revision,
    name: record.bank?.name ?? '', deleted: !record.bank,
    questionCount: record.bank?.questions.length ?? 0,
    contentHash: record.revision.toString(), updatedAt: record.updatedAt,
    deviceId: record.deviceId,
  })
  globalThis.fetch = async (input, options = {}) => {
    if (offline) throw new Error('network unavailable')
    const url = new URL(String(input))
    const path = url.pathname
    const method = options.method || 'GET'
    if (path === '/api/study/banks' && method === 'GET') return Response.json({ banks: [] })
    if (path === '/api/study/attempts/batch' && method === 'POST') return Response.json({ accepted: true })
    if (path === '/api/study/sessions/finish' && method === 'POST') return Response.json({ ok: true })
    if (path === '/api/study/personal-banks' && method === 'GET') {
      return Response.json({ banks: [...banks].map(([id, row]) => summary(id, row)) })
    }
    if (path.startsWith('/api/study/personal-banks/')) {
      const id = decodeURIComponent(path.slice('/api/study/personal-banks/'.length))
      const current = banks.get(id)
      if (method === 'GET') {
        return current ? Response.json({ ...summary(id, current), bank: current.bank })
          : Response.json({ error: 'personal_bank_not_found' }, { status: 404 })
      }
      const body = JSON.parse(options.body)
      if ((current?.revision ?? 0) !== body.expectedRevision) {
        return Response.json({ error: 'revision_conflict' }, { status: 409 })
      }
      const updated = { revision: (current?.revision ?? 0) + 1,
        bank: method === 'DELETE' ? null : structuredClone(body.bank),
        updatedAt: Date.now(), deviceId: body.deviceId }
      banks.set(id, updated)
      return Response.json(summary(id, updated))
    }
    if (path === '/api/study/progress' && method === 'GET') {
      return Response.json({ progress: [...progress].map(([bankKey, row]) => ({
        bankKey, revision: row.revision, updatedAt: row.updatedAt,
        deviceId: row.deviceId, deleted: !row.session,
      })) })
    }
    if (path.startsWith('/api/study/progress/')) {
      const id = decodeURIComponent(path.slice('/api/study/progress/'.length))
      const current = progress.get(id)
      if (method === 'GET') {
        return current ? Response.json({ ...current, bankKey: id, deleted: !current.session })
          : Response.json({ error: 'no_progress' }, { status: 404 })
      }
      const body = JSON.parse(options.body)
      if ((current?.revision ?? 0) !== body.expectedRevision) {
        return Response.json({ error: 'revision_conflict' }, { status: 409 })
      }
      const updated = { revision: (current?.revision ?? 0) + 1,
        session: method === 'DELETE' ? null : structuredClone(body.session),
        deviceId: body.deviceId, updatedAt: Date.now() }
      progress.set(id, updated)
      return Response.json({ ...updated, bankKey: id, deleted: !updated.session })
    }
    throw new Error(method + ' ' + path)
  }

  const vite = await createServer({ server: { middlewareMode: true },
    appType: 'custom', logLevel: 'silent' })
  try {
    const { usePracticeStore } = await vite.ssrLoadModule('/src/stores/practice.ts')
    const { saveStudySyncConfig } = await vite.ssrLoadModule('/src/services/studySync.ts')
    const stores = [new Storage(), new Storage()]
    const makeDevice = async (which, importedBank = null) => {
      globalThis.localStorage = stores[which]
      if (importedBank) localStorage.setItem('exameow-banks', JSON.stringify([importedBank]))
      saveStudySyncConfig({ baseUrl: 'https://my-study.example', token: 'my-secret' })
      setActivePinia(createPinia())
      const current = usePracticeStore()
      await current.syncStudyBanks()
      return current
    }
    const id = bank().id

    // Existing locally imported bank on A is uploaded without changing IDs.
    const A = await makeDevice(0, bank())
    assert.equal(banks.get(id).revision, 1)
    assert.equal(banks.get(id).bank.id, id)
    assert.equal(banks.get(id).bank.questions[1].answer, '√')
    globalThis.localStorage = stores[0]
    A.startSession(id, 'sequential')
    A.setAnswer('A')
    A.nextQuestion()
    await A.flushPracticeProgress()

    // B has never seen this file; it downloads bank and its complete session.
    const B = await makeDevice(1)
    assert.equal(B.banks.length, 1)
    assert.equal(B.getBank(id).id, id)
    assert.deepEqual(B.getBank(id).questions, bank().questions)
    assert.equal(B.getBankSession(id).currentIndex, 1)
    assert.equal(B.getBankSession(id).questions[0].userAnswer, 'A')
    assert.equal(B.activateBankSession(id), true)
    B.setAnswer('B')
    B.nextQuestion()
    globalThis.localStorage = stores[1]
    await B.flushPracticeProgress()
    globalThis.localStorage = stores[0]
    await A.syncPracticeProgress()
    assert.equal(A.getBankSession(id).currentIndex, 2)

    // Duplicate reimport on B must resolve to the already-downloaded bank ID.
    globalThis.localStorage = stores[1]
    B.importPreview = bank().questions
    B.importFileName = '数据结构第一章.csv'
    B.importSource = 'csv'
    assert.equal(B.confirmImport(), id)
    assert.equal(B.banks.length, 1)

    // A changes the bank offline, B changes it online from revision 1.
    globalThis.localStorage = stores[0]
    A.banks.find(item => item.id === id).name = '离线修改'
    localStorage.setItem('exameow-banks', JSON.stringify(A.banks))
    offline = true
    await A.syncPersonalBanks()
    offline = false

    globalThis.localStorage = stores[1]
    B.banks.find(item => item.id === id).name = '在线修改'
    localStorage.setItem('exameow-banks', JSON.stringify(B.banks))
    await B.syncPersonalBanks()
    assert.equal(banks.get(id).revision, 2)

    globalThis.localStorage = stores[0]
    await A.syncPersonalBanks()
    assert.ok(A.personalBankConflicts[id])
    assert.equal(A.getBank(id).name, '离线修改')
    assert.equal(banks.get(id).bank.name, '在线修改')
    await A.resolvePersonalBank(id, 'local')
    assert.equal(A.personalBankConflicts[id], undefined)
    assert.equal(banks.get(id).bank.name, '离线修改')

    globalThis.localStorage = stores[1]
    await B.syncPersonalBanks()
    assert.equal(B.getBank(id).name, '离线修改')

    // A has new unsynced answer edits when B deletes the bank. Never
    // silently erase them: show a conflict before accepting deletion.
    globalThis.localStorage = stores[0]
    A.getBankSession(id).questions[2].userAnswer = '本地未同步答案'
    localStorage.setItem('exameow-practice-session:' + encodeURIComponent(id),
      JSON.stringify(A.getBankSession(id)))
    globalThis.localStorage = stores[1]
    B.removeBank(id)
    await B.flushPersonalBanks()
    assert.equal(banks.get(id).bank, null)
    globalThis.localStorage = stores[0]
    await A.syncPersonalBanks()
    assert.ok(A.personalBankConflicts[id], 'unsynced progress must block automatic bank deletion')
    assert.equal(A.getBank(id).id, id)
    assert.equal(A.getBankSession(id).questions[2].userAnswer, '本地未同步答案')
    // The user can explicitly accept the cloud deletion after making a choice.
    await A.resolvePersonalBank(id, 'cloud')
    assert.equal(A.getBank(id), undefined)
    assert.equal(A.getBankSession(id), null)
    assert.ok(localStorage.getItem('exameow-personal-bank-backup:' + encodeURIComponent(id)))
    assert.ok(localStorage.getItem('exameow-practice-progress-backup:' + encodeURIComponent(id)))

    // Offline new imports remain local and upload when connectivity resumes.
    const another = bank('user-second-offline-bank')
    offline = true
    B.addBank(another)
    globalThis.localStorage = stores[1]
    await B.flushPersonalBanks()
    assert.equal(B.getBank(another.id).id, another.id)
    assert.equal(banks.has(another.id), false)
    offline = false
    await B.syncPersonalBanks()
    assert.equal(banks.get(another.id).bank.id, another.id)
    globalThis.localStorage = stores[0]
    await A.syncStudyBanks()
    assert.equal(A.getBank(another.id).name, another.name)
  } finally {
    await vite.close()
    globalThis.fetch = oldFetch
    if (oldStorage === undefined) delete globalThis.localStorage
    else globalThis.localStorage = oldStorage
  }
})
