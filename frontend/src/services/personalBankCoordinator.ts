/**
 * Coordinates user-owned bank content separately from study progress.
 * Offline local banks remain usable. Revisions prevent stale overwrites.
 * Sync order: public study banks -> personal banks -> practice progress.
 */
import { ref } from 'vue'
import type { QuestionBank } from '@exameow/shared'
import {
  type PersonalBankSummary,
  isPersonalBank, fingerprintPersonalBank, backupPersonalBank,
  loadPersonalBankMeta, savePersonalBankMeta,
  loadPersonalBankTombstones, setPersonalBankTombstone,
  listCloudPersonalBanks, getCloudPersonalBank,
  putCloudPersonalBank, deleteCloudPersonalBank,
} from './personalBankSync'
import { ProgressSyncError } from './practiceProgressSync'
import { isStudySyncConfigured } from './studySync'

interface BankCallbacks {
  list(): QuestionBank[]
  get(id: string): QuestionBank | undefined
  /** Apply a downloaded bank only after local fingerprint is re-checked. */
  apply(id: string, bank: QuestionBank | null, explicit?: boolean): boolean
}

const DEBOUNCE_MS = 1300

export function createPersonalBankCoordinator(callbacks: BankCallbacks) {
  const conflicts = ref<Record<string, PersonalBankSummary>>({})
  const error = ref('')
  const syncing = ref(false)
  const meta = loadPersonalBankMeta()
  const timers = new Map<string, ReturnType<typeof setTimeout>>()
  const inflight = new Map<string, Promise<void>>()
  let ready = false
  let ongoingSync: Promise<void> | null = null

  const localBank = (id: string) => {
    const bank = callbacks.get(id)
    return bank && isPersonalBank(bank) ? bank : undefined
  }

  function remember(id: string, revision: number, fingerprint: string) {
    meta[id] = { revision, fingerprint }
    if (!savePersonalBankMeta(meta)) {
      error.value = '无法保存题库同步状态，请检查浏览器存储空间。'
    }
  }

  function forgetConflict(id: string) {
    if (!conflicts.value[id]) return
    const updated = { ...conflicts.value }
    delete updated[id]
    conflicts.value = updated
  }

  function recordConflict(id: string, remote: PersonalBankSummary) {
    conflicts.value = { ...conflicts.value, [id]: remote }
  }

  function failed(e: unknown) {
    error.value = e instanceof Error ? e.message : String(e)
    console.warn('[personal-bank-sync]', error.value)
  }

  async function loadRemote(id: string) {
    try { return await getCloudPersonalBank(id) }
    catch (e) {
      if (e instanceof ProgressSyncError && e.status === 404) return null
      throw e
    }
  }

  function changedSinceAck(id: string): boolean {
    const bank = localBank(id)
    return !!loadPersonalBankTombstones()[id]
      || (!!bank && fingerprintPersonalBank(bank) !== meta[id]?.fingerprint)
  }

  function schedule(id: string) {
    if (!isStudySyncConfigured()) return
    const old = timers.get(id)
    if (old) clearTimeout(old)
    timers.set(id, setTimeout(() => {
      timers.delete(id)
      void uploadOne(id)
    }, DEBOUNCE_MS))
  }

  function localChanged(id: string) {
    setPersonalBankTombstone(id, false)
    if (!conflicts.value[id]) schedule(id)
  }

  function localDeleted(id: string) {
    if (!setPersonalBankTombstone(id, true)) {
      error.value = '无法保存待同步的题库删除操作，稍后请手动同步确认。'
    }
    if (!conflicts.value[id]) schedule(id)
  }

  async function uploadOne(id: string): Promise<void> {
    if (!ready || !isStudySyncConfigured() || conflicts.value[id]) return
    if (inflight.has(id)) {
      await inflight.get(id)
      return
    }
    const task = (async () => {
      const tombstone = !!loadPersonalBankTombstones()[id]
      const bank = localBank(id)
      if (!tombstone && !bank) return
      const currentHash = bank ? fingerprintPersonalBank(bank) : ''
      if (!tombstone && meta[id]?.fingerprint === currentHash) return
      const expected = meta[id]?.revision ?? 0
      try {
        const outcome = tombstone
          ? await deleteCloudPersonalBank(id, expected)
          : await putCloudPersonalBank(bank!, expected)
        remember(id, outcome.revision, tombstone ? '' : currentHash)
        if (tombstone) setPersonalBankTombstone(id, false)
        error.value = ''
        if (changedSinceAck(id)) schedule(id)
      } catch (e) {
        if (e instanceof ProgressSyncError && e.status === 409) {
          const remote = await loadRemote(id)
          if (remote) {
            const same = tombstone
              ? remote.deleted
              : !!remote.bank && !!bank
                && fingerprintPersonalBank(remote.bank) === currentHash
            if (same) {
              remember(id, remote.revision, tombstone ? '' : currentHash)
              if (tombstone) setPersonalBankTombstone(id, false)
            } else {
              recordConflict(id, remote)
            }
            return
          }
        }
        failed(e)
      }
    })()
    inflight.set(id, task)
    try { await task } finally {
      if (inflight.get(id) === task) inflight.delete(id)
    }
  }

  /**
   * Returns false if the browser has changed the bank in the meantime or if
   * a local backup/persistence operation fails.
   */
  function adoptRemote(id: string, remote: PersonalBankSummary,
                       bank: QuestionBank | null, originalHash: string | null,
                       explicit = false): boolean {
    const current = localBank(id)
    if ((current ? fingerprintPersonalBank(current) : null) !== originalHash
      || !!loadPersonalBankTombstones()[id]) return false
    if (bank && (bank.id !== id || !isPersonalBank(bank))) return false
    if (current && (!bank || fingerprintPersonalBank(current) !== fingerprintPersonalBank(bank))) {
      if (!backupPersonalBank(id, 'before-remote-bank-update', current)) return false
    }
    if (!callbacks.apply(id, bank, explicit)) return false
    const applied = localBank(id)
    remember(id, remote.revision, applied ? fingerprintPersonalBank(applied) : '')
    setPersonalBankTombstone(id, false)
    forgetConflict(id)
    return true
  }

  async function reconcile(id: string, summary?: PersonalBankSummary) {
    const bank = localBank(id)
    const localHash = bank ? fingerprintPersonalBank(bank) : null
    const known = meta[id]
    const deleted = !!loadPersonalBankTombstones()[id]

    if (!summary) {
      // A local bank may be deleted while its first upload is still in flight.
      // Wait before deciding that there is nothing to delete on the server.
      if (deleted && inflight.has(id)) {
        await inflight.get(id)
        if ((meta[id]?.revision ?? 0) > 0) {
          await uploadOne(id)
          return
        }
      }
      if (known && known.revision > 0) {
        // Preserve local data if the remote server was reset; no silent re-upload.
        recordConflict(id, {
          bankId: id, revision: 0, name: bank?.name ?? '',
          questionCount: 0, contentHash: '', updatedAt: 0, deviceId: '', deleted: true,
        })
      } else if (deleted) {
        // The bank never reached the server, so removing it needs no remote write.
        setPersonalBankTombstone(id, false)
      } else if (bank) await uploadOne(id)
      return
    }

    if (deleted) {
      if (known?.revision === summary.revision) await uploadOne(id)
      else if (summary.deleted && !bank) {
        remember(id, summary.revision, '')
        setPersonalBankTombstone(id, false)
      } else recordConflict(id, summary)
      return
    }

    if (bank && known?.revision === summary.revision) {
      if (localHash !== known.fingerprint) await uploadOne(id)
      return
    }

    // No local bank or a clean, acknowledged local copy: safe to adopt remote.
    if (!bank || known && localHash === known.fingerprint) {
      const remote = await loadRemote(id)
      if (!remote) return
      if (!adoptRemote(id, remote, remote.bank, localHash)) recordConflict(id, remote)
      return
    }

    // Freshly imported bank (or offline edits) collides with cloud data.
    const remote = await loadRemote(id)
    if (remote?.bank && fingerprintPersonalBank(remote.bank) === localHash) {
      remember(id, remote.revision, localHash)
      return
    }
    recordConflict(id, remote ?? summary)
  }

  async function sync(): Promise<void> {
    if (!isStudySyncConfigured()) return
    if (ongoingSync) return ongoingSync
    ongoingSync = (async () => {
      syncing.value = true
      error.value = ''
      try {
        const summaries = await listCloudPersonalBanks()
        ready = true
        const remote = new Map(summaries.map(s => [s.bankId, s]))
        const bankIds = new Set([
          ...callbacks.list().filter(isPersonalBank).map(b => b.id),
          ...Object.keys(loadPersonalBankTombstones()),
          ...remote.keys(),
        ])
        for (const id of bankIds) {
          if (conflicts.value[id]) continue
          try { await reconcile(id, remote.get(id)) }
          catch (e) { failed(e) }
        }
      } catch (e) { failed(e) }
      finally { syncing.value = false }
    })()
    try { await ongoingSync } finally { ongoingSync = null }
  }

  async function resolve(id: string, decision: 'cloud' | 'local'): Promise<void> {
    if (!isStudySyncConfigured() || !conflicts.value[id]) return
    const inFlight = inflight.get(id)
    if (inFlight) await inFlight
    syncing.value = true
    try {
      const latest = await loadRemote(id)
      const local = localBank(id)
      const originalHash = local ? fingerprintPersonalBank(local) : null
      if (decision === 'cloud') {
        if (latest && !adoptRemote(id, latest, latest.bank, originalHash, true)) {
          throw new Error('本地题库已发生变化，请重试。')
        }
        if (!latest && !local) forgetConflict(id)
      } else {
        if (latest?.bank && !backupPersonalBank(id, 'cloud-before-local-overwrite', latest.bank)) {
          throw new Error('无法保存云端旧版本备份，已取消覆盖。')
        }
        const deleted = !!loadPersonalBankTombstones()[id]
        if (!local && !deleted) throw new Error('没有可上传的本地题库。')
        const outcome = deleted
          ? await deleteCloudPersonalBank(id, latest?.revision ?? 0)
          : await putCloudPersonalBank(local!, latest?.revision ?? 0)
        remember(id, outcome.revision, deleted ? '' : originalHash!)
        if (deleted) setPersonalBankTombstone(id, false)
        forgetConflict(id)
        if (changedSinceAck(id)) schedule(id)
      }
      error.value = ''
    } catch (e) {
      if (e instanceof ProgressSyncError && e.status === 409) {
        const latest = await loadRemote(id)
        if (latest) recordConflict(id, latest)
      }
      failed(e)
    } finally { syncing.value = false }
  }

  async function flush() {
    if (!ready) await sync()
    const ids = [...timers.keys()]
    for (const id of ids) {
      clearTimeout(timers.get(id)!)
      timers.delete(id)
    }
    await Promise.all(ids.map(id => uploadOne(id)))
  }

  return { conflicts, error, syncing, sync, flush, resolve, localChanged, localDeleted }
}
