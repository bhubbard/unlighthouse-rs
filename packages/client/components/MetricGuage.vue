<script lang="ts" setup>
import { GAUGE_CONSTANTS } from '../constants'

const props = defineProps<{
  score?: number
  stripped?: boolean
  label?: string
}>()

// Direct prop access is more efficient than toRefs when not destructuring

const arc = ref(null)

const guageModifiers = computed(() => {
  let result = 'fail'
  if (props.score >= 0.9)
    result = 'pass'
  else if (props.score >= 0.5)
    result = 'average'

  return [
    `guage__wrapper--${result}`,
  ]
})

const gaugeColorClasses = computed(() => {
  if (props.score >= 0.9) {
    return 'text-success fill-current stroke-current'
  }
  else if (props.score >= 0.5) {
    return 'text-warning fill-current stroke-current'
  }
  else {
    return 'text-error fill-current stroke-current'
  }
})

const guageArcStyle = computed(() => {
  const { RADIUS, CIRCUMFERENCE, ROTATION_OFFSET } = GAUGE_CONSTANTS

  let offset = props.score * CIRCUMFERENCE - RADIUS / 2
  if (props.score === 1)
    offset = CIRCUMFERENCE

  return {
    opacity: props.score === 0 ? '0' : 1,
    transform: `rotate(${360 * ROTATION_OFFSET - 90}deg)`,
    strokeDasharray: `${Math.max(offset, 0)}, ${CIRCUMFERENCE}`,
  }
})

const accessibilityLabel = computed(() => {
  return `${props.label || 'Score'}: ${props.score === null ? 'Unknown' : Math.round(props.score * 100)}`
})
</script>

<template>
  <div
    v-if="props.stripped"
    :class="guageModifiers"
    role="progressbar"
    :aria-valuenow="props.score !== null ? Math.round(props.score * 100) : undefined"
    aria-valuemin="0"
    aria-valuemax="100"
    :aria-label="accessibilityLabel"
  >
    <audit-result :value="{ score: props.score, displayValue: Math.round(props.score * 100) }" />
  </div>

  <div
    v-else
    class="guage__wrapper guage__wrapper--huge group transition-transform duration-300 hover:scale-110"
    :class="[guageModifiers, gaugeColorClasses]"
    role="progressbar"
    :aria-valuenow="props.score !== null ? Math.round(props.score * 100) : undefined"
    aria-valuemin="0"
    aria-valuemax="100"
    :aria-label="accessibilityLabel"
  >
    <div class="guage__svg-wrapper relative">
      <svg class="guage" viewBox="0 0 120 120" role="img" :aria-label="accessibilityLabel">
        <circle
          class="guage-base"
          r="56"
          cx="60"
          cy="60"
          stroke-width="8"
        />
        <circle
          v-if="props.score !== null"
          ref="arc"
          class="guage-arc"
          r="56"
          cx="60"
          cy="60"
          stroke-width="8"
          :style="guageArcStyle"
        />
      </svg>
      <div
        class="font-5xl font-bold left-[50%] top-[50%] transform -translate-y-[50%] -translate-x-[50%] absolute text-mono font-mono"
        aria-hidden="true"
      >
        {{ props.score === null ? '?' : Math.round(props.score * 100) }}
      </div>
    </div>
    <div class="text-xs mt-2 font-medium opacity-80 group-hover:opacity-100 transition-opacity" aria-hidden="true">
      {{ props.label }}
    </div>
  </div>
</template>

<style scoped>
.guage__wrapper--huge {
  --gauge-circle-size: 40px;
}
.guage__wrapper {
  position: relative;
  display: flex;
  align-items: center;
  flex-direction: column;
  text-decoration: none;
  --transition-length: 1s;
  contain: content;
  will-change: transform, opacity;
}
.guage__svg-wrapper {
  position: relative;
  height: var(--gauge-circle-size);
}
.guage {
  stroke-linecap: round;
  width: var(--gauge-circle-size);
  height: var(--gauge-circle-size);
}
.guage-base {
  opacity: 0.1;
}
.guage-arc {
  fill: none;
  transform-origin: 50% 50%;
  animation: load-gauge var(--transition-length) ease forwards;
  animation-delay: 250ms;
}
</style>
