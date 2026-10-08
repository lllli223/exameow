<script setup lang="ts">
import { ref } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import type { SearchHit } from '@/utils/questionSearch'
import { ChevronDownIcon } from '@heroicons/vue/24/outline'
import { trueFalseOption } from '@/utils/answerGrading'

const props = defineProps<{ hit: SearchHit }>()
const i18n = useI18nStore()
const showAnalysis = ref(false)

const typeLabelKeys: Record<string, 'typeSingle' | 'typeMulti' | 'typeTrueFalse' | 'typeFillBlank' | 'typeShortAnswer'> = {
  single_choice: 'typeSingle',
  multi_choice: 'typeMulti',
  true_false: 'typeTrueFalse',
  fill_blank: 'typeFillBlank',
  short_answer: 'typeShortAnswer',
}

function isCorrectOption(idx: number): boolean {
  const q = props.hit.question
  if (q.type === 'single_choice' || q.type === 'multi_choice') {
    const letters = q.answer.toUpperCase().replace(/[^A-H]/g, '')
    return letters.includes(String.fromCharCode(65 + idx))
  }
  if (q.type === 'true_false') {
    const option = trueFalseOption(q.answer)
    return option === String.fromCharCode(65 + idx)
  }
  return false
}
</script>

<template>
  <div class="card-outlined p-4">
    <div class="flex items-center gap-2 flex-wrap mb-2">
      <span
        class="text-[11px] font-medium px-2 py-0.5 rounded-full"
        :style="{ backgroundColor: 'rgb(var(--md-secondary-container))', color: 'rgb(var(--md-on-secondary-container))' }"
      >
        {{ i18n.t(typeLabelKeys[hit.question.type] || 'typeShortAnswer') }}
      </span>
      <span
        class="text-[11px] px-2 py-0.5 rounded-full truncate max-w-[50%]"
        :style="{ backgroundColor: 'rgb(var(--md-surface-container-highest))', color: 'rgb(var(--md-on-surface-variant))' }"
      >
        {{ hit.bankName }}
      </span>
      <span class="text-[11px] ml-auto tabular-nums" style="color: rgb(var(--md-primary))">
        {{ i18n.t('searchSimilarity') }} {{ Math.round(hit.score * 100) }}%
      </span>
    </div>

    <p class="text-body-lg mb-2 whitespace-pre-wrap">{{ hit.question.stem }}</p>

    <div v-if="hit.question.options.length" class="space-y-1 mb-2">
      <div
        v-for="(opt, oi) in hit.question.options"
        :key="oi"
        class="text-body-md flex gap-2"
        :style="{
          color: isCorrectOption(oi) ? 'rgb(var(--md-primary))' : 'rgb(var(--md-on-surface-variant))',
          fontWeight: isCorrectOption(oi) ? 600 : 400,
        }"
      >
        <span>{{ String.fromCharCode(65 + oi) }}.</span>
        <span class="whitespace-pre-wrap">{{ opt }}</span>
      </div>
    </div>

    <div class="text-body-md mb-1">
      <span class="text-label-lg mr-2" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('tableAnswer') }}</span>
      <span class="font-semibold" style="color: rgb(var(--md-primary))">{{ hit.question.answer }}</span>
    </div>

    <template v-if="hit.question.analysis">
      <button
        class="flex items-center gap-1 text-sm font-medium"
        style="color: rgb(var(--md-primary))"
        @click="showAnalysis = !showAnalysis"
      >
        {{ showAnalysis ? i18n.t('searchHideAnalysis') : i18n.t('searchShowAnalysis') }}
        <ChevronDownIcon
          class="w-4 h-4 transition-transform"
          :style="{ transform: showAnalysis ? 'rotate(180deg)' : 'rotate(0)' }"
        />
      </button>
      <p v-if="showAnalysis" class="text-body-md mt-2 whitespace-pre-wrap" style="color: rgb(var(--md-on-surface-variant))">
        {{ hit.question.analysis }}
      </p>
    </template>
  </div>
</template>
