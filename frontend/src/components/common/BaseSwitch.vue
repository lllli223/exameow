<script setup lang="ts">
import { CheckIcon } from '@heroicons/vue/24/outline'

const props = defineProps<{
  modelValue: boolean
  disabled?: boolean
}>()

const emit = defineEmits<{ (e: 'update:modelValue', v: boolean): void }>()

function toggle() {
  if (props.disabled) return
  emit('update:modelValue', !props.modelValue)
}

function onKeydown(e: KeyboardEvent) {
  if (e.repeat) return
  if (e.key === ' ' || e.key === 'Enter') {
    e.preventDefault()
    toggle()
  }
}
</script>

<template>
  <button
    type="button"
    role="switch"
    :aria-checked="modelValue"
    :aria-disabled="disabled || undefined"
    :disabled="disabled"
    class="group relative inline-flex shrink-0 items-center w-[52px] h-8 rounded-full outline-none focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[rgb(var(--md-primary))] transition-colors duration-200 cursor-pointer disabled:opacity-40 disabled:pointer-events-none"
    :style="{
      backgroundColor: modelValue ? 'rgb(var(--md-primary))' : 'rgb(var(--md-surface-container-highest))',
      border: modelValue ? '2px solid rgb(var(--md-primary))' : '2px solid rgb(var(--md-outline))',
    }"
    @click="toggle"
    @keydown="onKeydown"
  >
    <span
      class="pointer-events-none absolute inset-0 rounded-full opacity-0 transition-opacity duration-200 group-hover:opacity-[0.08] group-active:opacity-[0.12]"
      :style="{ backgroundColor: modelValue ? 'rgb(var(--md-on-primary))' : 'rgb(var(--md-on-surface))' }"
    />
    <span
      class="pointer-events-none absolute top-1/2 -translate-y-1/2 flex items-center justify-center rounded-full shadow-sm transition-all duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)]"
      :style="{
        width: modelValue ? '24px' : '16px',
        height: modelValue ? '24px' : '16px',
        left: modelValue ? '24px' : '8px',
        backgroundColor: modelValue ? 'rgb(var(--md-on-primary))' : 'transparent',
        border: modelValue ? 'none' : '2px solid rgb(var(--md-outline))',
        color: 'rgb(var(--md-primary))',
      }"
    >
      <CheckIcon v-if="modelValue" class="w-4 h-4" />
    </span>
  </button>
</template>
