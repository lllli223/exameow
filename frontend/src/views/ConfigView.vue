<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useConfigStore } from '@/stores/config'

import { useI18nStore } from '@/stores/i18n'
import { isCloudflare, isTauri } from '@/utils/platform'
import {
  loadStudySyncConfig,
  saveStudySyncConfig,
  flushAttempts,
  testStudyConnection,
  pendingCount,
  syncing,
} from '@/services/studySync'
import BaseCombobox from '@/components/common/BaseCombobox.vue'
import { ServerIcon, KeyIcon, CloudArrowDownIcon, CpuChipIcon, CheckCircleIcon, EyeIcon, EyeSlashIcon, CheckIcon, ArrowRightIcon, ArrowLeftIcon, CloudIcon, GlobeAltIcon } from '@heroicons/vue/24/outline'

const configStore = useConfigStore()
const router = useRouter()
const i18n = useI18nStore()

const showKey = ref(false)
const saveSuccess = ref(false)
const saveError = ref('')

const configFetchError = ref('')
const configFetching = ref(false)

// ── Study Sync (self-hosted study API; independent of the AI API config) ──
const savedSyncConfig = loadStudySyncConfig()
const syncBaseUrl = ref(savedSyncConfig.baseUrl)
const syncToken = ref(savedSyncConfig.token)
const showSyncToken = ref(false)
const syncSaveSuccess = ref(false)
const syncTesting = ref(false)
const syncTestSuccess = ref('')
const syncTestError = ref('')
const syncFlushInfo = ref('')
const syncFlushError = ref('')
const syncPendingCount = computed(() => pendingCount.value)
const syncFlushing = computed(() => syncing.value)

function handleSyncSave() {
  saveStudySyncConfig({ baseUrl: syncBaseUrl.value, token: syncToken.value })
  syncSaveSuccess.value = true
  setTimeout(() => { syncSaveSuccess.value = false }, 2500)
  // Saving may enable sync — opportunistically drain the outbox (non-blocking)
  flushAttempts()
}

async function handleSyncTest() {
  syncTesting.value = true
  syncTestSuccess.value = ''
  syncTestError.value = ''
  try {
    const result = await testStudyConnection(syncBaseUrl.value, syncToken.value)
    if (result.ok) syncTestSuccess.value = result.message
    else syncTestError.value = result.message
  } finally {
    syncTesting.value = false
  }
}

async function handleSyncFlush() {
  syncFlushInfo.value = ''
  syncFlushError.value = ''
  const result = await flushAttempts()
  if (result.remaining > 0) {
    syncFlushError.value = `${i18n.t('syncTestFail')} (${i18n.t('syncPending', { n: result.remaining })})`
  } else if (result.sent > 0) {
    syncFlushInfo.value = i18n.t('syncFlushed', { n: result.sent })
  }
}

const showEndpointAndAuth = computed(() => {
  if (isTauri()) return true
  if (isCloudflare()) return configStore.aiProvider === 'custom'
  return configStore.aiProvider !== 'server'
})

const fetchModelsDisabled = computed(() => {
  if (isCloudflare()) return configStore.aiProvider === 'custom' && (!configStore.endpoint || !configStore.apiKey)
  if (isTauri()) return !configStore.endpoint || !configStore.apiKey
  if (configStore.aiProvider === 'server') return false
  return !configStore.endpoint || !configStore.apiKey
})

async function handleFetchModels() {
  configFetchError.value = ''
  configFetching.value = true
  try {
    await configStore.fetchModels()
    configStore.model = ''
  } catch (e: any) { configFetchError.value = e.message || String(e) } finally { configFetching.value = false }
}


async function handleSave() {
  saveError.value = ''
  try {
    await configStore.save()
    saveSuccess.value = true
    setTimeout(() => saveSuccess.value = false, 2500)
  } catch (e: any) { saveError.value = e.message || String(e) }
}
</script>

<template>
  <div class="max-w-3xl mx-auto">
    <div class="flex items-center gap-3 mb-6">
      <button class="btn-icon" @click="router.push('/mine')">
        <ArrowLeftIcon class="w-5 h-5" />
      </button>
      <div>
        <h1 class="text-display-sm font-bold tracking-tight">{{ i18n.t('configTitle') }}</h1>
      </div>
    </div>

    <!-- CF: Provider toggle -->
    <div v-if="isCloudflare()" class="card-filled p-5 mb-4 shadow-sm border border-[rgb(var(--md-outline-variant)/0.3)]">
      <label class="text-label-md font-semibold block mb-3" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('configAiProvider') }}</label>
      <div class="flex items-center gap-3">
        <button
          class="btn-tonal text-sm !px-5 !py-2.5"
          :class="{ 'btn-filled': configStore.aiProvider === 'cf-free' }"
          @click="configStore.setProvider('cf-free')"
        >
          <CloudIcon class="w-4 h-4" />
          <span>{{ i18n.t('configCfFree') }}</span>
        </button>
        <button
          class="btn-tonal text-sm !px-5 !py-2.5"
          :class="{ 'btn-filled': configStore.aiProvider === 'custom' }"
          @click="configStore.setProvider('custom')"
        >
          <ServerIcon class="w-4 h-4" />
          <span>{{ i18n.t('configCustomApi') }}</span>
        </button>
      </div>
      <p v-if="configStore.aiProvider === 'cf-free'" class="text-body-sm mt-3" style="color: rgb(var(--md-on-surface-variant))">
        {{ i18n.t('configCfFreeDesc') }}
      </p>
      <p v-else class="text-body-sm mt-3" style="color: rgb(var(--md-on-surface-variant))">
        {{ i18n.t('configCustomApiDesc') }}
      </p>
    </div>

    <!-- HTTP/Web: Server env AI vs custom API -->
    <div v-if="!isCloudflare() && !isTauri()" class="card-filled p-5 mb-4 shadow-sm border border-[rgb(var(--md-outline-variant)/0.3)]">
      <label class="text-label-md font-semibold block mb-3" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('configAiProvider') }}</label>
      <div class="flex items-center gap-3">
        <button
          v-if="configStore.serverInfo?.has_env_ai"
          class="btn-tonal text-sm !px-5 !py-2.5"
          :class="{ 'btn-filled': configStore.aiProvider === 'server' }"
          @click="configStore.setProvider('server')"
        >
          <CpuChipIcon class="w-4 h-4" />
          <span>{{ i18n.t('configServerAi') }}</span>
        </button>
        <button
          class="btn-tonal text-sm !px-5 !py-2.5"
          :class="{ 'btn-filled': configStore.aiProvider !== 'server' }"
          @click="configStore.setProvider('custom')"
        >
          <ServerIcon class="w-4 h-4" />
          <span>{{ i18n.t('configCustomApi') }}</span>
        </button>
      </div>
      <p v-if="configStore.aiProvider === 'server'" class="text-body-sm mt-3" style="color: rgb(var(--md-on-surface-variant))">
        {{ i18n.t('configServerAiDesc') }}<template v-if="configStore.serverInfo?.endpoint"> · {{ configStore.serverInfo.endpoint }}</template>
      </p>
      <p v-else class="text-body-sm mt-3" style="color: rgb(var(--md-on-surface-variant))">
        {{ i18n.t('configCustomApiDesc') }}
      </p>
    </div>

    <!-- Endpoint (custom API or non-CF) -->
    <div v-if="showEndpointAndAuth" class="card-filled p-5 sm:p-6 mb-4 shadow-sm border border-[rgb(var(--md-outline-variant)/0.3)]">
      <label class="text-label-md font-semibold block mb-3" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('configSectionEndpoint') }}</label>
      <div class="relative">
        <ServerIcon class="absolute left-3.5 top-1/2 -translate-y-1/2 w-5 h-5 z-10" style="color: rgb(var(--md-on-surface-variant))" />
        <input
          v-model="configStore.endpoint"
          placeholder="https://<your_api_url>/v1"
          class="input-outlined !pl-11 !rounded-2xl !py-3"
        />
      </div>
    </div>

    <!-- Auth (custom API or non-CF) -->
    <div v-if="showEndpointAndAuth" class="card-filled p-5 sm:p-6 mb-4 shadow-sm border border-[rgb(var(--md-outline-variant)/0.3)]">
      <label class="text-label-md font-semibold block mb-3" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('configSectionAuth') }}</label>
      <div class="relative">
        <KeyIcon class="absolute left-3.5 top-1/2 -translate-y-1/2 w-5 h-5 z-10" style="color: rgb(var(--md-on-surface-variant))" />
        <input
          v-model="configStore.apiKey"
          :type="showKey ? 'text' : 'password'"
          :placeholder="i18n.t('configApiKey')"
          class="input-outlined !pl-11 !pr-11 !rounded-2xl !py-3"
        />
        <button class="absolute right-3 top-1/2 -translate-y-1/2 btn-icon !w-8 !h-8" @click="showKey = !showKey">
          <EyeSlashIcon v-if="showKey" class="w-4 h-4" />
          <EyeIcon v-else class="w-4 h-4" />
        </button>
      </div>
    </div>

    <!-- Model -->
    <div class="card-filled p-5 sm:p-6 mb-4 shadow-sm border border-[rgb(var(--md-outline-variant)/0.3)]">
      <label class="text-label-md font-semibold block mb-3" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('configSectionModel') }}</label>
      <div class="flex flex-col sm:flex-row items-center gap-3">
        <button
          class="btn-tonal shrink-0 text-sm !h-12 !px-4 !rounded-2xl"
          :disabled="fetchModelsDisabled"
          @click="handleFetchModels"
        >
          <CloudArrowDownIcon class="w-4 h-4" />
          <span>{{ configFetching ? '...' : i18n.t('configFetchModels') }}</span>
        </button>
        <div class="flex-1 relative w-full">
          <CpuChipIcon class="absolute left-3.5 top-3.5 w-5 h-5 z-10 pointer-events-none" style="color: rgb(var(--md-on-surface-variant))" />
          <BaseCombobox
            :model-value="configStore.model"
            :options="configStore.models.map(m => ({ value: m.id, label: m.id }))"
            :placeholder="i18n.t('configEnterModel')"
            class="[&_input]:!pl-11 [&_input]:!rounded-2xl [&_input]:!py-3"
            @update:model-value="configStore.model = $event"
          />
        </div>
      </div>
    </div>

    <!-- Study Sync (self-hosted study API) -->
    <div class="card-filled p-5 sm:p-6 mb-4 shadow-sm border border-[rgb(var(--md-outline-variant)/0.3)]">
      <label class="text-label-md font-semibold block mb-1" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('syncTitle') }}</label>
      <p class="text-body-sm mb-4" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('syncDesc') }}</p>

      <div class="relative mb-3">
        <GlobeAltIcon class="absolute left-3.5 top-1/2 -translate-y-1/2 w-5 h-5 z-10" style="color: rgb(var(--md-on-surface-variant))" />
        <input
          v-model="syncBaseUrl"
          placeholder="https://your-study-api.example.com"
          class="input-outlined !pl-11 !rounded-2xl !py-3"
        >
      </div>

      <div class="relative">
        <KeyIcon class="absolute left-3.5 top-1/2 -translate-y-1/2 w-5 h-5 z-10" style="color: rgb(var(--md-on-surface-variant))" />
        <input
          v-model="syncToken"
          :type="showSyncToken ? 'text' : 'password'"
          :placeholder="i18n.t('syncToken')"
          class="input-outlined !pl-11 !pr-11 !rounded-2xl !py-3"
          autocomplete="off"
        >
        <button class="absolute right-3 top-1/2 -translate-y-1/2 btn-icon !w-8 !h-8" @click="showSyncToken = !showSyncToken">
          <EyeSlashIcon v-if="showSyncToken" class="w-4 h-4" />
          <EyeIcon v-else class="w-4 h-4" />
        </button>
      </div>

      <div class="flex flex-wrap items-center gap-2 mt-4">
        <button class="btn-tonal text-sm !h-10 !px-4" @click="handleSyncSave">
          <CheckIcon class="w-4 h-4" />
          <span>{{ i18n.t('configSave') }}</span>
        </button>
        <button
          class="btn-tonal text-sm !h-10 !px-4"
          :disabled="!syncBaseUrl.trim() || syncTesting"
          @click="handleSyncTest"
        >
          <span>{{ syncTesting ? '...' : i18n.t('syncTestConnection') }}</span>
        </button>
        <button
          class="btn-tonal text-sm !h-10 !px-4"
          :disabled="syncPendingCount === 0 || syncFlushing"
          @click="handleSyncFlush"
        >
          <span>{{ syncFlushing ? '...' : i18n.t('syncFlush') }}</span>
        </button>
        <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">
          {{ i18n.t('syncPending', { n: syncPendingCount }) }}
        </span>
        <Transition name="fade">
          <span v-if="syncSaveSuccess" class="flex items-center gap-1 text-sm font-medium" style="color: rgb(var(--md-primary))">
            <CheckCircleIcon class="w-4 h-4" /> {{ i18n.t('configSaved') }}
          </span>
        </Transition>
      </div>

      <div v-if="syncTestSuccess" class="mt-3 text-sm font-medium flex items-center gap-1" style="color: rgb(var(--md-primary))">
        <CheckCircleIcon class="w-4 h-4" /> {{ i18n.t('syncTestOk') }} ({{ syncTestSuccess }})
      </div>
      <div v-if="syncTestError" class="mt-3 text-sm" style="color: rgb(var(--md-error))">
        {{ i18n.t('syncTestFail') }} ({{ syncTestError }})
      </div>
      <div v-if="syncFlushInfo" class="mt-3 text-sm font-medium flex items-center gap-1" style="color: rgb(var(--md-primary))">
        <CheckCircleIcon class="w-4 h-4" /> {{ syncFlushInfo }}
      </div>
      <div v-if="syncFlushError" class="mt-3 text-sm" style="color: rgb(var(--md-error))">
        {{ syncFlushError }}
      </div>
    </div>

    <Transition name="scale">
      <div
        v-if="configFetchError"
        class="mb-4 px-4 py-3 rounded-2xl text-sm flex items-center gap-2"
        style="background-color: rgb(var(--md-error-container)); color: rgb(var(--md-on-error-container))"
      >
        <span>{{ configFetchError }}</span>
      </div>
    </Transition>

    <!-- ========= Unified Save + CTA ========= -->
    <div class="flex items-center justify-center gap-3 mt-8">
      <button class="btn-filled !h-12 !px-8 shadow-md" :disabled="!configStore.configured" @click="handleSave">
        <CheckIcon class="w-5 h-5" />
        <span>{{ i18n.t('configSave') }}</span>
      </button>

      <button
        class="btn-tonal !h-12 !px-8 shadow-sm"
        :disabled="!configStore.configured"
        @click="router.push('/generate')"
      >
        <span>{{ i18n.t('configReadyCta') }}</span>
        <ArrowRightIcon class="w-5 h-5" />
      </button>

      <Transition name="fade">
        <div v-if="saveSuccess" class="flex items-center gap-2 text-sm font-medium" style="color: rgb(var(--md-primary))">
          <CheckCircleIcon class="w-5 h-5" /> {{ i18n.t('configSaved') }}
        </div>
      </Transition>
    </div>

    <Transition name="scale">
      <div
        v-if="saveError"
        class="mt-4 px-4 py-3 rounded-2xl text-sm"
        style="background-color: rgb(var(--md-error-container)); color: rgb(var(--md-on-error-container))"
      >
        {{ saveError }}
      </div>
    </Transition>
  </div>
</template>
