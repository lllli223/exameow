<script setup lang="ts">
import { ref } from 'vue'
import { useI18nStore } from '@/stores/i18n'
import type { WrongSort } from '@exameow/shared'
import {
  ArrowDownIcon,
  ArrowUpIcon,
  ClockIcon,
  ExclamationTriangleIcon,
  PlayIcon,
} from '@heroicons/vue/24/outline'

const props = defineProps<{
  reviewCount: number
}>()

const emit = defineEmits<{
  (e: 'start', sort: WrongSort): void
  (e: 'close'): void
}>()

const i18n = useI18nStore()

const sorts: { value: WrongSort; label: string; icon: any }[] = [
  { value: 'count-desc', label: i18n.t('wrongSortCountDesc'), icon: ArrowDownIcon },
  { value: 'count-asc', label: i18n.t('wrongSortCountAsc'), icon: ArrowUpIcon },
  { value: 'time-desc', label: i18n.t('wrongSortTimeDesc'), icon: ClockIcon },
  { value: 'time-asc', label: i18n.t('wrongSortTimeAsc'), icon: ClockIcon },
]

const selected = ref<WrongSort>('count-desc')
</script>

<template>
  <div class="scrim flex items-center justify-center p-4" @click.self="emit('close')">
    <div class="card-elevated w-full max-w-sm p-5">
      <div class="flex items-center gap-3 mb-4">
        <div
          class="w-10 h-10 rounded-xl flex items-center justify-center shrink-0"
          :style="{ backgroundColor: 'rgb(var(--md-error-container))' }"
        >
          <ExclamationTriangleIcon class="w-5 h-5" :style="{ color: 'rgb(var(--md-on-error-container))' }" />
        </div>
        <div>
          <div class="text-title-sm" :style="{ color: 'rgb(var(--md-on-surface))' }">
            {{ i18n.t('wrongModeTitle') }}
          </div>
          <div class="text-body-sm" :style="{ color: 'rgb(var(--md-on-surface-variant))' }">
            {{ i18n.t('practiceQuestionUnit', { n: reviewCount }) }}
          </div>
        </div>
      </div>

      <div class="text-label-sm mb-2" :style="{ color: 'rgb(var(--md-on-surface-variant))' }">
        {{ i18n.t('wrongSortTitle') }}
      </div>

      <div class="space-y-1.5 mb-4">
        <button
          v-for="s in sorts"
          :key="s.value"
          class="w-full text-left p-3 rounded-xl border transition-all duration-200 flex items-center gap-3"
          :style="{
            borderColor: selected === s.value ? 'rgb(var(--md-primary))' : 'rgb(var(--md-outline-variant))',
            backgroundColor: selected === s.value ? 'rgba(var(--md-primary), 0.08)' : 'transparent',
          }"
          @click="selected = s.value"
        >
          <component :is="s.icon" class="w-4 h-4" :style="{ color: 'rgb(var(--md-on-surface-variant))' }" />
          <span class="text-sm" :style="{ color: 'rgb(var(--md-on-surface))' }">{{ s.label }}</span>
          <div
            class="w-4 h-4 rounded-full border-2 flex items-center justify-center ml-auto"
            :style="{
              borderColor: selected === s.value ? 'rgb(var(--md-primary))' : 'rgb(var(--md-outline-variant))',
            }"
          >
            <div
              v-if="selected === s.value"
              class="w-2 h-2 rounded-full"
              :style="{ backgroundColor: 'rgb(var(--md-primary))' }"
            />
          </div>
        </button>
      </div>

      <div class="flex gap-3">
        <button class="btn-outlined flex-1" @click="emit('close')">
          {{ i18n.t('btnBack') }}
        </button>
        <button class="btn-filled flex-1" @click="emit('start', selected)">
          <PlayIcon class="w-4 h-4" />
          {{ i18n.t('practiceStartBtn') }}
        </button>
      </div>
    </div>
  </div>
</template>
