<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { addFileInputs, removeFileInput, clearFileInputs, fileInputsRef, fileNamesRef } from '@/stores/fileInput'
import { isMobileDevice } from '@/utils/platform'
import { DocumentArrowUpIcon, DocumentTextIcon, CameraIcon, XMarkIcon } from '@heroicons/vue/24/outline'

const i18n = useI18nStore()
const props = defineProps<{ isTauri: boolean }>()

const webFiles = ref<File[]>([])
const webFileInput = ref<HTMLInputElement>()
const cameraInput = ref<HTMLInputElement>()
const dialogReady = ref(false)
const isDragOver = ref(false)
const showCamera = isMobileDevice()

const DOC_EXTENSIONS = ['txt', 'md', 'markdown', 'docx', 'pdf', 'pptx', 'html', 'htm', 'odt', 'epub', 'csv', 'xlsx', 'xlsm', 'xls', 'ods']
const IMAGE_EXTENSIONS = ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp']
const CODE_EXTENSIONS = ['py', 'js', 'ts', 'jsx', 'tsx', 'mjs', 'cjs', 'java', 'c', 'cpp', 'cc', 'h', 'hpp', 'cs', 'go', 'rs', 'rb', 'php', 'swift', 'kt', 'kts', 'sql', 'sh', 'bash', 'zsh', 'bat', 'ps1', 'json', 'yaml', 'yml', 'toml', 'xml', 'ini', 'cfg', 'conf', 'log', 'tex', 'r', 'lua', 'pl', 'scala', 'dart', 'vue', 'svelte', 'css', 'scss', 'less']
const acceptAttr = [...DOC_EXTENSIONS, ...IMAGE_EXTENSIONS, ...CODE_EXTENSIONS].map((e) => '.' + e).join(',')

onMounted(async () => {
  if (props.isTauri) {
    try {
      const mod = await import('@tauri-apps/plugin-dialog')
      ;(window as any).__tauriDialogOpen = mod.open
      dialogReady.value = true
      console.log('[FileUploader] dialog plugin loaded OK')
    } catch (e) {
      dialogReady.value = false
      console.warn('[FileUploader] dialog plugin FAILED to load:', e)
    }
  }
  console.log('[FileUploader] isTauri:', props.isTauri, 'dialogReady:', dialogReady.value)
})

const fileNames = computed(() => fileNamesRef.value)

function clearAll() {
  webFiles.value = []
  clearFileInputs()
}

function clearOne(index: number) {
  if (fileInputsRef.value.length <= index) return
  const removed = fileInputsRef.value[index]
  if (removed instanceof File) {
    const webIdx = webFiles.value.indexOf(removed)
    if (webIdx >= 0) webFiles.value.splice(webIdx, 1)
  }
  removeFileInput(index)
}

async function pick() {
  if (props.isTauri && dialogReady.value) {
    const openFn = (window as any).__tauriDialogOpen
    if (!openFn) return
    const result: any = await openFn({
      multiple: true,
      filters: [
        { name: 'Documents', extensions: DOC_EXTENSIONS },
        { name: 'Images', extensions: IMAGE_EXTENSIONS },
        { name: 'Text & Code', extensions: CODE_EXTENSIONS },
        { name: 'All Files', extensions: ['*'] },
      ],
    })
    console.log('[FileUploader] open result:', typeof result, result)
    if (result) {
      const results = Array.isArray(result) ? result : [result]
      const paths = results.map((r: any) => typeof r === 'string' ? r : r.path)
      console.log('[FileUploader] normalized paths:', paths)
      addFileInputs(paths)
    }
  } else {
    console.log('[FileUploader] fallback to web file input (dialogReady:', dialogReady.value, ')')
    webFileInput.value?.click()
  }
}

function onWebFilesChange(event: Event) {
  const input = event.target as HTMLInputElement
  if (input.files?.length) {
    const newFiles = Array.from(input.files)
    webFiles.value.push(...newFiles)
    addFileInputs(newFiles)
    input.value = ''
  }
}

function pickCamera() {
  cameraInput.value?.click()
}

function onCameraChange(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  input.value = ''
  const now = new Date()
  const dateStr = now.toLocaleDateString(i18n.locale === 'zh' ? 'zh-CN' : 'en-US', { year: 'numeric', month: '2-digit', day: '2-digit' })
  const timeStr = now.toLocaleTimeString(i18n.locale === 'zh' ? 'zh-CN' : 'en-US', { hour: '2-digit', minute: '2-digit' })
  const ext = file.name.split('.').pop() || 'jpg'
  const name = `${i18n.t('genTakePhoto')} ${dateStr} ${timeStr}.${ext}`
  const photoFile = new File([file], name, { type: file.type })
  webFiles.value.push(photoFile)
  addFileInputs([photoFile])
}

function onDragOver(e: DragEvent) { e.preventDefault(); isDragOver.value = true }
function onDragLeave() { isDragOver.value = false }
function onDrop(e: DragEvent) {
  e.preventDefault()
  isDragOver.value = false
  if (e.dataTransfer?.files?.length) {
    const newFiles = Array.from(e.dataTransfer.files)
    webFiles.value.push(...newFiles)
    addFileInputs(newFiles)
  }
}
</script>

<template>
  <div
    class="text-center h-full w-full flex flex-col items-center justify-center"
    @dragover="onDragOver"
    @dragleave="onDragLeave"
    @drop="onDrop"
  >
    <!-- Drop Zone -->
    <button
      v-if="fileNames.length === 0"
      class="flex flex-col items-center gap-4 group cursor-pointer w-full h-full min-h-[200px] sm:min-h-[220px] justify-center rounded-[32px] transition-all duration-300 border-2"
      :style="{
        borderColor: isDragOver
          ? 'rgb(var(--md-primary))'
          : 'rgb(var(--md-outline-variant) / 0.4)',
        backgroundColor: isDragOver
          ? 'rgba(var(--md-primary) / 0.08)'
          : 'rgb(var(--md-surface-container-low))',
      }"
      @click="pick"
    >
      <div
        class="w-20 h-20 rounded-[28px] flex items-center justify-center transition-transform duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)] group-hover:scale-110 group-active:scale-95 shadow-md"
        :style="{ backgroundColor: 'rgb(var(--md-primary-container))' }"
      >
        <DocumentArrowUpIcon class="w-10 h-10 transition-transform duration-300 group-hover:-translate-y-0.5" style="color: rgb(var(--md-on-primary-container))" />
      </div>
      <div class="px-4">
        <div class="text-title-md font-bold tracking-tight" style="color: rgb(var(--md-primary))">{{ i18n.t('genSelectFile') }}</div>
        <div class="text-body-sm mt-1.5 font-medium" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('genFileHint') }}</div>
        <div class="text-[11px] mt-1" style="color: rgb(var(--md-on-surface-muted))">{{ i18n.t('practiceMultiFileHint') }}</div>
      </div>

      <input
        ref="webFileInput"
        type="file"
        :accept="acceptAttr"
        multiple
        class="hidden"
        @change="onWebFilesChange"
      />
    </button>

    <input
      ref="cameraInput"
      type="file"
      accept="image/*"
      capture="environment"
      class="hidden"
      @change="onCameraChange"
    />

    <button
      v-if="showCamera && fileNames.length === 0"
      class="btn-tonal mt-4 gap-2 shadow-sm"
      @click="pickCamera"
    >
      <CameraIcon class="w-5 h-5" />
      <span>{{ i18n.t('genTakePhoto') }}</span>
    </button>

    <!-- File List -->
    <div v-if="fileNames.length > 0" class="w-full">
      <div class="flex items-center justify-between mb-3 px-1">
        <span class="text-label-md font-semibold" style="color: rgb(var(--md-on-surface-variant))">
          {{ i18n.t('practiceFileCount', { n: fileNames.length }) }}
        </span>
        <div class="flex items-center gap-1">
          <button v-if="showCamera" class="btn-tonal !h-8 !text-xs !px-3 gap-1" @click="pickCamera">
            <CameraIcon class="w-3.5 h-3.5" />
            <span>{{ i18n.t('genTakePhoto') }}</span>
          </button>
          <button class="btn-tonal !h-8 !text-xs !px-3" @click="pick">{{ i18n.t('practiceAddFile') }}</button>
        </div>
      </div>

      <TransitionGroup name="list" tag="div" class="space-y-2 max-h-[280px] overflow-y-auto px-0.5">
        <div
          v-for="(name, i) in fileNames"
          :key="i"
          class="flex items-center gap-3 px-3.5 py-2.5 rounded-2xl transition-all duration-200 shadow-sm border border-[rgb(var(--md-outline-variant)/0.3)]"
          :style="{ backgroundColor: 'rgb(var(--md-surface-container))' }"
        >
          <CameraIcon v-if="name.includes('Photo') || name.includes('拍照') || name.startsWith(i18n.t('genTakePhoto'))" class="w-4 h-4 shrink-0" style="color: rgb(var(--md-primary))" />
          <DocumentTextIcon v-else class="w-4 h-4 shrink-0" style="color: rgb(var(--md-primary))" />
          <span class="text-sm font-medium truncate flex-1 text-left" style="color: rgb(var(--md-on-surface))">{{ name }}</span>
          <button
            class="shrink-0 rounded-full p-1.5 transition-colors duration-200 hover:bg-black/10 dark:hover:bg-white/10"
            @click.stop="clearOne(i)"
          >
            <XMarkIcon class="w-4 h-4" style="color: rgb(var(--md-on-surface-variant))" />
          </button>
        </div>
      </TransitionGroup>

      <div class="mt-4 flex justify-center">
        <button class="btn-text !h-8 !text-xs !px-3" style="color: rgb(var(--md-error))" @click="clearAll">
           {{ i18n.t('practiceClearAll') }}
        </button>
      </div>

      <input
        ref="webFileInput"
        type="file"
        :accept="acceptAttr"
        multiple
        class="hidden"
        @change="onWebFilesChange"
      />
    </div>
  </div>
</template>
