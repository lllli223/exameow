import { defineStore } from 'pinia'
import { ref, computed, watch } from 'vue'
import type { AIConfig, ModelInfo, ReasoningEffort, TokenParameter } from '@exameow/shared'
import { api } from '@/api'
import { isCloudflare, isTauri } from '@/utils/platform'
import type { ServerConfigInfo } from '@/api/http'

import { DEFAULT_CF_MODEL } from '@/api/cf-models'
import { fetchModelsFromEndpoint } from '@/utils/modelList'
import { normalizeEndpoint, withV1Suffix } from '@/utils/endpoint'

export type AIProvider = 'cf-free' | 'custom' | 'server'

export const useConfigStore = defineStore('config', () => {
  const endpoint = ref('')
  const apiKey = ref('')
  const model = ref('')
  const maxTokens = ref<number | null>(null)
  const tokenParameter = ref<TokenParameter>('max_tokens')
  const temperature = ref<number | null>(0.7)
  const reasoningEffort = ref<ReasoningEffort | ''>('')
  const extraPrompt = ref('')
  const retries = ref(0)
  const timeoutSeconds = ref<number | null>(null)
  const models = ref<ModelInfo[]>([])
  const loading = ref(false)
  const aiProvider = ref<AIProvider>('cf-free')
  const serverInfo = ref<ServerConfigInfo | null>(null)

  const configured = computed(() => {
    if (!isCloudflare() && !isTauri() && aiProvider.value === 'server') {
      return true
    }
    if (!isCloudflare()) {
      return !!endpoint.value && !!apiKey.value && !!model.value
    }
    if (aiProvider.value === 'cf-free') {
      return !!model.value
    }
    return !!endpoint.value && !!apiKey.value && !!model.value
  })

  async function loadSaved() {
    const saved = await api.loadConfig()
    if (saved) {
      if (saved.endpoint) endpoint.value = saved.endpoint
      if (saved.api_key) apiKey.value = saved.api_key
      model.value = saved.model
      maxTokens.value = typeof saved.max_tokens === 'number' ? saved.max_tokens : null
      tokenParameter.value = saved.token_parameter === 'max_completion_tokens' ? 'max_completion_tokens' : 'max_tokens'
      if (typeof saved.temperature === 'number') temperature.value = saved.temperature
      else if (saved.omit_temperature === true) temperature.value = null
      reasoningEffort.value = saved.reasoning_effort ?? ''
      extraPrompt.value = saved.extra_prompt ?? ''
      retries.value = typeof saved.retries === 'number' ? saved.retries : 0
      timeoutSeconds.value = typeof saved.timeout_seconds === 'number' ? saved.timeout_seconds : null
    }
    if (isCloudflare()) {
      const provider = localStorage.getItem('exameow_ai_provider')
      if (provider === 'custom' || provider === 'cf-free') {
        aiProvider.value = provider
      }
      if (!model.value) {
        endpoint.value = 'cloudflare-worker'
        apiKey.value = 'cloudflare-worker'
      }
      return
    }
    if (isTauri()) return
    serverInfo.value = await api.getServerInfo()
    const storedProvider = localStorage.getItem('exameow_ai_provider')
    if (storedProvider === 'server' && serverInfo.value?.has_env_ai) {
      aiProvider.value = 'server'
      if (!model.value && serverInfo.value?.model) model.value = serverInfo.value.model
      return
    }
    if (endpoint.value || apiKey.value || model.value) return
    if (serverInfo.value?.has_env_ai) {
      aiProvider.value = 'server'
      if (serverInfo.value.model) model.value = serverInfo.value.model
      localStorage.setItem('exameow_ai_provider', 'server')
    }
  }

  async function fetchModels() {
    loading.value = true
    try {
      if (isCloudflare() && aiProvider.value === 'cf-free') {
        models.value = await api.getModels({ endpoint: '', api_key: '', model: '' })
        return
      }
      if (!isCloudflare() && !isTauri() && aiProvider.value === 'server') {
        models.value = await api.getModels({ endpoint: '', api_key: '', model: '' })
        return
      }
      endpoint.value = normalizeEndpoint(endpoint.value)
      if (!endpoint.value || !apiKey.value) return
      try {
        if (isCloudflare() && aiProvider.value === 'custom') {
          models.value = await fetchModelsFromEndpoint(endpoint.value, apiKey.value)
        } else {
          models.value = await api.getModels({ endpoint: endpoint.value, api_key: apiKey.value, model: '' })
        }
      } catch (firstError) {
        const candidate = withV1Suffix(endpoint.value)
        if (!candidate) throw firstError
        if (isCloudflare() && aiProvider.value === 'custom') {
          models.value = await fetchModelsFromEndpoint(candidate, apiKey.value)
        } else {
          models.value = await api.getModels({ endpoint: candidate, api_key: apiKey.value, model: '' })
        }
        endpoint.value = candidate
      }
    } catch (e: any) {
      throw new Error(e.message || String(e))
    } finally {
      loading.value = false
    }
  }

  async function save() {
    localStorage.setItem('exameow_ai_provider', aiProvider.value)
    if (!isCloudflare() && !isTauri() && aiProvider.value === 'server') {
      return
    }
    endpoint.value = normalizeEndpoint(endpoint.value)
    await api.saveConfig(buildConfig())
  }

  function buildConfig(): AIConfig {
    return {
      endpoint: endpoint.value,
      api_key: apiKey.value,
      model: model.value,
      max_tokens: maxTokens.value ?? undefined,
      token_parameter: tokenParameter.value,
      temperature: temperature.value ?? undefined,
      omit_temperature: temperature.value === null,
      reasoning_effort: reasoningEffort.value || undefined,
      extra_prompt: extraPrompt.value.trim() || undefined,
      retries: retries.value,
      timeout_seconds: timeoutSeconds.value ?? undefined,
    }
  }

  function getConfig(): AIConfig {
    if (!isCloudflare() && !isTauri() && aiProvider.value === 'server') {
      return { ...buildConfig(), endpoint: '', api_key: '' }
    }
    return buildConfig()
  }

  function setProvider(provider: AIProvider) {
    aiProvider.value = provider
    models.value = []
    if (provider === 'cf-free' && !model.value) {
      model.value = DEFAULT_CF_MODEL
    }
    if (provider === 'server' && !model.value && serverInfo.value?.model) {
      model.value = serverInfo.value.model
    }
  }

  return { endpoint, apiKey, model, maxTokens, tokenParameter, temperature, reasoningEffort, extraPrompt, retries, timeoutSeconds, models, loading, configured, aiProvider, serverInfo, loadSaved, fetchModels, save, getConfig, setProvider }
})
