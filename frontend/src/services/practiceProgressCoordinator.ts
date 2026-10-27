/**
 * Revisioned per-bank practice sync. The browser stays authoritative while
 * offline; conflicting edits are never silently overwritten.
 */
import { ref } from 'vue'
import type { QuestionBank, PracticeSession } from '@exameow/shared'
import {
  type CloudProgressSummary,
  ProgressSyncError,
  listCloudPracticeProgress, getCloudPracticeProgress,
  putCloudPracticeProgress, deleteCloudPracticeProgress,
  loadCloudProgressMeta, saveCloudProgressMeta,
  loadProgressDeleted, setProgressDeleted,
  fingerprintSession, backupPracticeProgress,
} from './practiceProgressSync'
import { isStudySyncConfigured } from './studySync'

interface Callbacks {
  getBanks(): QuestionBank[]
  getSession(bankId: string): PracticeSession | null
  /** Returns false if a snapshot cannot safely be applied. */
  applyRemote(bankId: string, session: PracticeSession | null): boolean
}

const DEBOUNCE_MS = 1200

export function createPracticeProgressCoordinator(callbacks: Callbacks) {
  const conflicts = ref<Record<string, CloudProgressSummary>>({})
  const syncing = ref(false)
  const error = ref('')
  const meta = loadCloudProgressMeta()
  const timers = new Map<string, ReturnType<typeof setTimeout>>()
  const inflight = new Map<string, Promise<void>>()
  let ready = false
  let reconciliation: Promise<void> | null = null

  const findBank = (id: string) => callbacks.getBanks().find(bank => bank.id === id)
  const cloudKey = (bank: QuestionBank) => bank.remoteKey || bank.id

  function remember(bankId: string, revision: number, hash: string) {
    meta[bankId] = { revision, fingerprint: hash }
    saveCloudProgressMeta(meta)
  }

  function clearConflict(bankId: string) {
    if (!conflicts.value[bankId]) return
    const next = { ...conflicts.value }
    delete next[bankId]
    conflicts.value = next
  }

  function setConflict(bankId: string, remote: CloudProgressSummary) {
    conflicts.value = { ...conflicts.value, [bankId]: remote }
  }

  function handleError(err: unknown) {
    error.value = err instanceof Error ? err.message : String(err)
    console.warn('[practice-progress]', error.value)
  }

  async function getCurrent(key: string) {
    try { return await getCloudPracticeProgress(key) }
    catch (err) {
      if (err instanceof ProgressSyncError && err.status === 404) return null
      throw err
    }
  }

  function dirty(bankId: string) {
    if (loadProgressDeleted()[bankId]) return true
    const current = callbacks.getSession(bankId)
    return !!current && fingerprintSession(current) !== meta[bankId]?.fingerprint
  }

  /** After a local write, avoid immediate network calls on every keystroke. */
  function schedule(bankId: string, delay = DEBOUNCE_MS) {
    if (!isStudySyncConfigured()) return
    const previous = timers.get(bankId)
    if (previous) clearTimeout(previous)
    timers.set(bankId, setTimeout(() => {
      timers.delete(bankId)
      void pushOne(bankId)
    }, delay))
  }

  function localChanged(bankId: string) {
    setProgressDeleted(bankId, false)
    if (!conflicts.value[bankId]) schedule(bankId)
  }

  function localDeleted(bankId: string) {
    setProgressDeleted(bankId, true)
    if (!conflicts.value[bankId]) schedule(bankId, 0)
  }

  async function pushOne(bankId: string): Promise<void> {
    if (!ready || !isStudySyncConfigured() || conflicts.value[bankId]) return
    if (inflight.has(bankId)) {
      // Current in-flight write re-schedules if the local contents changed.
      await inflight.get(bankId)
      return
    }
    const bank = findBank(bankId)
    if (!bank) return
    const task = (async () => {
      const isDeleted = !!loadProgressDeleted()[bankId]
      const session = callbacks.getSession(bankId)
      if (!isDeleted && !session) return
      const hash = session ? fingerprintSession(session) : ''
      if (!isDeleted && hash === meta[bankId]?.fingerprint) return
      const expected = meta[bankId]?.revision ?? 0
      try {
        const response = isDeleted
          ? await deleteCloudPracticeProgress(cloudKey(bank), expected)
          : await putCloudPracticeProgress(cloudKey(bank), session!, expected)
        remember(bankId, response.revision, isDeleted ? '' : hash)
        if (isDeleted) setProgressDeleted(bankId, false)
        error.value = ''
        // Do not drop writes that occurred during the network request.
        if (dirty(bankId)) schedule(bankId)
      } catch (err) {
        if (err instanceof ProgressSyncError && err.status === 409) {
          const remote = await getCurrent(cloudKey(bank))
          if (remote) {
            const remoteHash = remote.session ? fingerprintSession(remote.session) : ''
            if (isDeleted && remote.deleted || !isDeleted && remoteHash === hash) {
              remember(bankId, remote.revision, remoteHash)
              if (isDeleted) setProgressDeleted(bankId, false)
            } else {
              setConflict(bankId, remote)
            }
            return
          }
        }
        handleError(err)
      }
    })()
    inflight.set(bankId, task)
    try { await task }
    finally { if (inflight.get(bankId) === task) inflight.delete(bankId) }
  }

  /**
   * A response to GET may arrive after the user starts answering. Check the
   * local fingerprint again before applying it.
   */
  function applyRemote(bankId: string, session: PracticeSession | null,
                       revision: number, originalHash: string | null): boolean {
    const existing = callbacks.getSession(bankId)
    const nowHash = existing ? fingerprintSession(existing) : null
    if (nowHash !== originalHash || !!loadProgressDeleted()[bankId]) return false
    if (existing && session) backupPracticeProgress(bankId, 'local-before-cloud-update', existing)
    if (!callbacks.applyRemote(bankId, session)) return false
    const applied = callbacks.getSession(bankId)
    remember(bankId, revision, applied ? fingerprintSession(applied) : '')
    setProgressDeleted(bankId, false)
    clearConflict(bankId)
    return true
  }

  async function reconcileBank(bank: QuestionBank, summary?: CloudProgressSummary) {
    const bankId = bank.id
    const session = callbacks.getSession(bankId)
    const localHash = session ? fingerprintSession(session) : null
    const known = meta[bankId]
    const pendingDelete = !!loadProgressDeleted()[bankId]

    if (!summary) {
      if (known && known.revision > 0) {
        // Database reset or deleted remotely; never assume old revisions are valid.
        setConflict(bankId, { bankKey: cloudKey(bank), revision: 0,
          updatedAt: 0, deviceId: '', deleted: true })
      } else if (session || pendingDelete) await pushOne(bankId)
      return
    }

    if (pendingDelete) {
      if (known?.revision === summary.revision) await pushOne(bankId)
      else setConflict(bankId, summary)
      return
    }
    if (session && known?.revision === summary.revision) {
      if (localHash !== known.fingerprint) await pushOne(bankId)
      return
    }
    if (!session || known && localHash === known.fingerprint) {
      const remote = await getCurrent(cloudKey(bank))
      if (!remote) return
      if (!applyRemote(bankId, remote.session, remote.revision, localHash)) {
        setConflict(bankId, remote)
      }
      return
    }
    // Fresh local state with cloud history, or concurrent offline edits.
    const remote = await getCurrent(cloudKey(bank))
    if (remote && remote.session && fingerprintSession(remote.session) === localHash) {
      remember(bankId, remote.revision, localHash)
      return
    }
    setConflict(bankId, remote ?? summary)
  }

  /** Reconcile on page load, manual sync, or network reconnect. No polling. */
  async function sync(): Promise<void> {
    if (!isStudySyncConfigured()) return
    if (reconciliation) return reconciliation
    reconciliation = (async () => {
      syncing.value = true
      error.value = ''
      try {
        const summaries = await listCloudPracticeProgress()
        ready = true
        const byKey = new Map(summaries.map(item => [item.bankKey, item]))
        for (const bank of callbacks.getBanks()) {
          try { await reconcileBank(bank, byKey.get(cloudKey(bank))) }
          catch (err) { handleError(err) }
        }
      } catch (err) { handleError(err) }
      finally { syncing.value = false }
    })()
    try { await reconciliation } finally { reconciliation = null }
  }

  /**
   * Resolve concurrent edits explicitly. Both branches remain recoverable:
   * a browser backup of the discarded version and server revision history.
   */
  async function resolve(bankId: string, choice: 'local' | 'cloud'): Promise<void> {
    const bank = findBank(bankId)
    if (!bank || !conflicts.value[bankId] || !isStudySyncConfigured()) return
    if (inflight.has(bankId)) await inflight.get(bankId)
    syncing.value = true
    try {
      const remote = await getCurrent(cloudKey(bank))
      const local = callbacks.getSession(bankId)
      const localHash = local ? fingerprintSession(local) : null
      if (choice === 'cloud') {
        if (remote && !applyRemote(bankId, remote.session, remote.revision, localHash)) {
          error.value = '本机进度已发生变化，请重试。'
        }
      } else {
        if (remote?.session) backupPracticeProgress(bankId, 'cloud-before-local-overwrite', remote.session)
        const isDeleted = !!loadProgressDeleted()[bankId]
        if (!isDeleted && !local) return
        const response = isDeleted
          ? await deleteCloudPracticeProgress(cloudKey(bank), remote?.revision ?? 0)
          : await putCloudPracticeProgress(cloudKey(bank), local!, remote?.revision ?? 0)
        remember(bankId, response.revision, isDeleted ? '' : localHash!)
        if (isDeleted) setProgressDeleted(bankId, false)
        clearConflict(bankId)
        if (dirty(bankId)) schedule(bankId)
      }
      if (!conflicts.value[bankId]) error.value = ''
    } catch (err) {
      if (err instanceof ProgressSyncError && err.status === 409) {
        const remote = await getCurrent(cloudKey(bank))
        if (remote) setConflict(bankId, remote)
      }
      handleError(err)
    } finally {
      syncing.value = false
    }
  }

  async function flush() {
    const pendingIds = [...timers.keys()]
    for (const bankId of pendingIds) {
      clearTimeout(timers.get(bankId)!)
      timers.delete(bankId)
    }
    await Promise.all(pendingIds.map(pushOne))
  }

  return { conflicts, syncing, error, sync, flush, resolve, localChanged, localDeleted }
}
