<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import { usePracticeStore } from '@/stores/practice'
import { useWrongQuestionsStore } from '@/stores/wrongQuestions'
import type { WrongSort, WrongQuestionEntry } from '@exameow/shared'
import { isReviewEntry, reviewActivityAt, sortReviewEntries } from '@/utils/reviewEntries'
import {
  ArrowDownIcon,
  ArrowUpIcon,
  ClockIcon,
  XMarkIcon,
  ExclamationTriangleIcon,
  TrashIcon,
  DocumentTextIcon,
} from '@heroicons/vue/24/outline'

const props = defineProps<{
  bankId: string
}>()

const emit = defineEmits<{
  (e: 'close'): void
}>()

const i18n = useI18nStore()
const practiceStore = usePracticeStore()
const wrongStore = useWrongQuestionsStore()

const sort = ref<WrongSort>('count-desc')
const showClearConfirm = ref(false)

const bank = computed(() => practiceStore.getBank(props.bankId))
const entries = computed(() => {
  const map = wrongStore.getBankEntryMap(props.bankId)
  return sortReviewEntries(
    Object.values(map).filter(e => isReviewEntry(e) && bank.value?.questions.some(q => q.id === e.questionId)),
    sort.value,
  )
})

function getQuestionStem(entry: WrongQuestionEntry): string {
  const q = bank.value?.questions.find(q => q.id === entry.questionId)
  return q?.stem ?? i18n.t('practiceQuestionGone')
}

function formatDate(ts: number): string {
  const d = new Date(ts)
  return `${d.getFullYear()}/${String(d.getMonth() + 1).padStart(2, '0')}/${String(d.getDate()).padStart(2, '0')}`
}

function handleRemove(questionId: string) {
  wrongStore.removeWrong(props.bankId, questionId)
}

function handleClear() {
  wrongStore.clearBank(props.bankId)
  showClearConfirm.value = false
}

const sortOptions: { value: WrongSort; label: string; icon: any }[] = [
  { value: 'count-desc', label: i18n.t('wrongSortCountDesc'), icon: ArrowDownIcon },
  { value: 'count-asc', label: i18n.t('wrongSortCountAsc'), icon: ArrowUpIcon },
  { value: 'time-desc', label: i18n.t('wrongSortTimeDesc'), icon: ClockIcon },
  { value: 'time-asc', label: i18n.t('wrongSortTimeAsc'), icon: ClockIcon },
]
</script>

<template>
  <div class="scrim flex items-center justify-center p-4" @click.self="emit('close')">
    <div class="card-elevated w-full max-w-lg max-h-[80vh] flex flex-col">
      <!-- Header -->
      <div class="flex items-center justify-between p-4 border-b" :style="{ borderColor: 'rgb(var(--md-outline-variant))' }">
        <div>
          <div class="text-title-sm" :style="{ color: 'rgb(var(--md-on-surface))' }">
            {{ i18n.t('wrongManagerTitle') }} - {{ bank?.name ?? '' }}
          </div>
          <div class="text-body-sm" :style="{ color: 'rgb(var(--md-on-surface-variant))' }">
            {{ i18n.t('practiceQuestionUnit', { n: entries.length }) }}
          </div>
        </div>
        <button class="btn-icon !w-8 !h-8" @click="emit('close')">
          <XMarkIcon class="w-5 h-5" />
        </button>
      </div>

      <!-- Sort bar -->
      <div v-if="entries.length > 0" class="flex items-center gap-2 px-4 py-2 border-b" :style="{ borderColor: 'rgb(var(--md-outline-variant))' }">
        <template v-for="opt in sortOptions" :key="opt.value">
          <button
            class="text-xs px-2.5 py-1 rounded-full transition-all duration-200"
            :style="{
              backgroundColor: sort === opt.value ? 'rgb(var(--md-secondary-container))' : 'transparent',
              color: sort === opt.value ? 'rgb(var(--md-on-secondary-container))' : 'rgb(var(--md-on-surface-variant))',
            }"
            @click="sort = opt.value"
          >
            {{ opt.label }}
          </button>
        </template>
        <button
          class="btn-icon !w-7 !h-7 ml-auto"
          :style="{ color: 'rgb(var(--md-error))' }"
          @click="showClearConfirm = true"
        >
          <TrashIcon class="w-4 h-4" />
        </button>
      </div>

      <!-- List -->
      <div class="overflow-y-auto flex-1 p-4">
        <div
          v-if="entries.length === 0"
          class="text-center py-8"
        >
          <DocumentTextIcon class="w-10 h-10 mx-auto mb-3" :style="{ color: 'rgb(var(--md-on-surface-muted))' }" />
          <div class="text-body-sm" :style="{ color: 'rgb(var(--md-on-surface-variant))' }">
            {{ i18n.t('wrongManagerEmpty') }}
          </div>
        </div>

        <div v-else class="space-y-2">
          <div
            v-for="entry in entries"
            :key="entry.questionId"
            class="card-outlined p-3 flex items-start gap-3"
          >
            <div class="flex-1 min-w-0">
              <div class="text-sm line-clamp-2" :style="{ color: 'rgb(var(--md-on-surface))' }">
                {{ getQuestionStem(entry) }}
              </div>
              <div class="flex items-center gap-3 mt-1.5 text-xs" :style="{ color: 'rgb(var(--md-on-surface-variant))' }">
                <span v-if="entry.wrongCount > 0" class="flex items-center gap-1">
                  <ExclamationTriangleIcon class="w-3 h-3" :style="{ color: 'rgb(var(--md-error))' }" />
                  {{ i18n.t('wrongTimesCount', { n: entry.wrongCount }) }}
                </span>
                <span v-if="entry.flagged" class="flex items-center gap-1" style="color: rgb(var(--md-primary))">
                  {{ i18n.t('practiceFlagUncertain') }}
                </span>
                <span class="flex items-center gap-1">
                  <ClockIcon class="w-3 h-3" />
                  {{ formatDate(reviewActivityAt(entry)) }}
                </span>
              </div>
            </div>
            <button
              class="btn-icon !w-7 !h-7 shrink-0"
              :style="{ color: 'rgb(var(--md-on-surface-variant))' }"
              @click="handleRemove(entry.questionId)"
            >
              <XMarkIcon class="w-4 h-4" />
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>

  <!-- Clear Confirm Dialog -->
  <Transition name="scale">
    <div v-if="showClearConfirm" class="scrim flex items-center justify-center p-4" @click.self="showClearConfirm = false">
      <div class="card-elevated w-full max-w-sm p-5 text-center">
        <TrashIcon class="w-10 h-10 mx-auto mb-3" :style="{ color: 'rgb(var(--md-error))' }" />
        <div class="text-title-sm mb-1" :style="{ color: 'rgb(var(--md-on-surface))' }">
          {{ i18n.t('wrongClearConfirm') }}
        </div>
        <p class="text-body-sm mb-4" :style="{ color: 'rgb(var(--md-on-surface-variant))' }">
          {{ i18n.t('wrongClearConfirmMsg') }}
        </p>
        <div class="flex gap-3">
          <button class="btn-outlined flex-1" @click="showClearConfirm = false">{{ i18n.t('btnBack') }}</button>
          <button
            class="btn-filled flex-1"
            :style="{ backgroundColor: 'rgb(var(--md-error))', color: 'rgb(var(--md-on-error))' }"
            @click="handleClear"
          >
            {{ i18n.t('wrongClearBtn') }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>
