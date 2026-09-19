<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18nStore } from '@/stores/i18n'
import { usePracticeStore } from '@/stores/practice'
import {
  loadStudySyncConfig,
  saveStudySyncConfig,
  flushAttempts,
  testStudyConnection,
  pendingCount,
  syncing,
} from '@/services/studySync'
import {
  ArrowLeftIcon,
  CheckCircleIcon,
  CheckIcon,
  EyeIcon,
  EyeSlashIcon,
  GlobeAltIcon,
  KeyIcon,
  PlayIcon,
} from '@heroicons/vue/24/outline'

const router = useRouter()
const i18n = useI18nStore()
const practiceStore = usePracticeStore()

const saved = loadStudySyncConfig()
const baseUrl = ref(saved.baseUrl)
const token = ref(saved.token)
const showToken = ref(false)
const saving = ref(false)
const testing = ref(false)
const success = ref('')
const error = ref('')
const pending = computed(() => pendingCount.value)
const flushing = computed(() => syncing.value)

async function syncBanks() {
  const result = await practiceStore.syncStudyBanks()
  return result.added + result.updated
}

async function handleSave() {
  if (!baseUrl.value.trim() || !token.value.trim()) return
  saving.value = true
  success.value = ''
  error.value = ''
  try {
    saveStudySyncConfig({ baseUrl: baseUrl.value, token: token.value })
    const bankCount = await syncBanks()
    void flushAttempts()
    success.value = bankCount > 0
      ? i18n.t('syncFlushed', { n: bankCount })
      : i18n.t('configSaved')
  } catch (e: any) {
    error.value = e?.message ?? String(e)
  } finally {
    saving.value = false
  }
}

async function handleTest() {
  testing.value = true
  success.value = ''
  error.value = ''
  try {
    const result = await testStudyConnection(baseUrl.value, token.value)
    if (!result.ok) {
      error.value = result.message
      return
    }
    saveStudySyncConfig({ baseUrl: baseUrl.value, token: token.value })
    const bankCount = await syncBanks()
    success.value = bankCount > 0
      ? `${i18n.t('syncTestOk')} · ${i18n.t('syncFlushed', { n: bankCount })}`
      : i18n.t('syncTestOk')
  } catch (e: any) {
    error.value = e?.message ?? String(e)
  } finally {
    testing.value = false
  }
}

async function handleFlush() {
  success.value = ''
  error.value = ''
  try {
    const bankCount = await syncBanks()
    const result = await flushAttempts()
    if (result.remaining > 0) {
      error.value = `${i18n.t('syncTestFail')} · ${i18n.t('syncPending', { n: result.remaining })}`
    } else {
      success.value = i18n.t('syncFlushed', { n: bankCount + result.sent })
    }
  } catch (e: any) {
    error.value = e?.message ?? String(e)
  }
}
</script>

<template>
  <div class="max-w-3xl mx-auto pb-8">
    <div class="flex items-center gap-3 mb-6">
      <button class="btn-icon" @click="router.push('/mine')">
        <ArrowLeftIcon class="w-5 h-5" />
      </button>
      <div>
        <h1 class="text-display-sm font-bold tracking-tight">{{ i18n.t('syncTitle') }}</h1>
        <p class="text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">
          {{ i18n.t('syncDesc') }}
        </p>
      </div>
    </div>

    <div class="card-filled p-5 sm:p-6 shadow-sm border border-[rgb(var(--md-outline-variant)/0.3)]">
      <div class="relative mb-3">
        <GlobeAltIcon class="absolute left-3.5 top-1/2 -translate-y-1/2 w-5 h-5 z-10" style="color: rgb(var(--md-on-surface-variant))" />
        <input
          v-model="baseUrl"
          placeholder="https://your-study-api.example.com"
          class="input-outlined !pl-11 !rounded-2xl !py-3"
        >
      </div>

      <div class="relative">
        <KeyIcon class="absolute left-3.5 top-1/2 -translate-y-1/2 w-5 h-5 z-10" style="color: rgb(var(--md-on-surface-variant))" />
        <input
          v-model="token"
          :type="showToken ? 'text' : 'password'"
          :placeholder="i18n.t('syncToken')"
          class="input-outlined !pl-11 !pr-11 !rounded-2xl !py-3"
          autocomplete="off"
        >
        <button class="absolute right-3 top-1/2 -translate-y-1/2 btn-icon !w-8 !h-8" @click="showToken = !showToken">
          <EyeSlashIcon v-if="showToken" class="w-4 h-4" />
          <EyeIcon v-else class="w-4 h-4" />
        </button>
      </div>

      <div class="flex flex-wrap items-center gap-2 mt-4">
        <button class="btn-tonal text-sm !h-10 !px-4" :disabled="saving || !baseUrl.trim() || !token.trim()" @click="handleSave">
          <CheckIcon class="w-4 h-4" />
          <span>{{ saving ? '...' : i18n.t('configSave') }}</span>
        </button>
        <button class="btn-tonal text-sm !h-10 !px-4" :disabled="testing || !baseUrl.trim() || !token.trim()" @click="handleTest">
          <span>{{ testing ? '...' : i18n.t('syncTestConnection') }}</span>
        </button>
        <button class="btn-tonal text-sm !h-10 !px-4" :disabled="flushing || !baseUrl.trim() || !token.trim()" @click="handleFlush">
          <span>{{ flushing ? '...' : i18n.t('syncFlush') }}</span>
        </button>
        <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
          {{ i18n.t('syncPending', { n: pending }) }}
        </span>
      </div>

      <div v-if="success" class="mt-4 text-sm font-medium flex items-center gap-2" style="color: rgb(var(--md-primary))">
        <CheckCircleIcon class="w-4 h-4 shrink-0" />
        <span>{{ success }}</span>
      </div>
      <div v-if="error" class="mt-4 text-sm break-all" style="color: rgb(var(--md-error))">
        {{ i18n.t('syncTestFail') }} · {{ error }}
      </div>

      <button class="btn-filled mt-6 !h-11 !px-5" @click="router.push('/practice')">
        <PlayIcon class="w-4 h-4" />
        <span>{{ i18n.t('navPractice') }}</span>
      </button>
    </div>
  </div>
</template>
