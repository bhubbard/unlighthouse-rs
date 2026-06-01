<script setup lang="ts">
import UDropdownMenu from '@nuxt/ui/components/DropdownMenu.vue'
import { useTitle } from '@vueuse/core'
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, ref, unref } from 'vue'
import { EXCLUDED_CATEGORIES } from './constants'
import {
  apiUrl,
  isOffline,
  isStatic,
  refreshScanMeta,
  rescanRoute,
  tabs,
  throttle,
  website,
  wsConnect,
} from './logic'
import { useUnlighthouseStore } from './stores/unlighthouse'

const store = useUnlighthouseStore()
const LighthouseThreeD = defineAsyncComponent(() => import('./components/LighthouseThreeD.vue'))
const payload = typeof window !== 'undefined' ? (window as any).__unlighthouse_payload : {}

if (!isStatic) {
  let refreshInterval: NodeJS.Timeout | null = null

  onMounted(() => {
    wsConnect().catch((error) => {
      console.warn('Failed to establish server connection:', error)
    })

    refreshInterval = setInterval(() => {
      refreshScanMeta()
    }, 5000)

    store.fetchCrux()
  })

  onUnmounted(() => {
    if (refreshInterval) {
      clearInterval(refreshInterval)
      refreshInterval = null
    }
  })
}

function openPsi(report) {
  window.open(`https://pagespeed.web.dev/report?url=${encodeURIComponent(report.route.url)}`, '_blank')
}

// Computed for complex template expressions
const shouldShowCategoryScore = computed(() => (category, key) => {
  return !EXCLUDED_CATEGORIES.includes(category.label) && store.categoryScores[key - 1] > 0
})

const shouldShowCruxTab = computed(() => {
  return store.crux && !store.cruxError && store.crux.exists !== false
})

const filteredTabs = computed(() => {
  if (!shouldShowCruxTab.value) {
    return tabs.filter(tab => tab.label !== 'CrUX')
  }
  return tabs
})

const getDropdownActions = computed(() => (report) => {
  const actions = []

  if (report.report) {
    actions.push({
      icon: 'i-heroicons-document-text',
      label: 'Open Lighthouse Report',
      description: 'Lighthouse HTML report is opened in a modal.',
      onSelect: () => store.openLighthouseReportIframeModal(report),
      disabled: false,
    })
  }

  actions.push({
    icon: 'i-heroicons-arrow-path',
    label: 'Rescan Route',
    description: 'Crawl the route again and generate a fresh report.',
    onSelect: () => rescanRoute(report.route),
    disabled: unref(isOffline) || unref(isStatic),
  })

  if (report.report) {
    actions.push({
      icon: 'i-mdi-speedometer',
      label: 'Run PageSpeed Insights',
      description: 'Get more accurate performance data by running a PageSpeed Insights test.',
      onSelect: () => openPsi(report),
      disabled: false,
    })
  }

  return actions
})

const _appName = (window as any).__unlighthouse_payload?.appName ?? 'Unlighthouse'
useTitle(`${website.replace(/https?:\/\/(www.)?/, '')} | ${_appName}`)

const tabListEl = ref<HTMLElement | null>(null)

function setActiveTab(key: number) {
  if (typeof document !== 'undefined' && (document as any).startViewTransition) {
    (document as any).startViewTransition(() => {
      store.activeTab = key
    })
  } else {
    store.activeTab = key
  }
}

function onTabKeydown(e: KeyboardEvent, key: number) {
  const count = filteredTabs.value.length
  let next = key
  if (e.key === 'ArrowDown' || e.key === 'ArrowRight')
    next = (key + 1) % count
  else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft')
    next = (key - 1 + count) % count
  else if (e.key === 'Home')
    next = 0
  else if (e.key === 'End')
    next = count - 1
  else
    return
  e.preventDefault()
  setActiveTab(next)
  nextTick(() => {
    tabListEl.value?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus()
  })
}
</script>

<template>
  <UApp>
    <div class="text-gray-700 dark:text-gray-200 overflow-y-hidden max-h-screen h-screen grid grid-rows-[min-content_1fr]">
      <NavBar />
      <main class="xl:flex mt-2 mb-2" :aria-busy="store.shouldShowWaitingState">
        <div class="flex justify-between max-h-[95%] flex-col xl:ml-3 mx-3 mr-0 w-full xl:mr-5 xl:w-[250px] xl:mb-0">
          <div>
            <div ref="tabListEl" role="tablist" aria-orientation="vertical" class="xl:block xl:space-x-0 flex space-x-2 mb-3">
              <btn-tab
                v-for="(category, key) in filteredTabs"
                :key="key"
                role="tab"
                :aria-selected="store.activeTab === key"
                :tabindex="store.activeTab === key ? 0 : -1"
                :selected="store.activeTab === key"
                @click="setActiveTab(key)"
                @keydown="onTabKeydown($event, key)"
                class="flex-col !items-stretch !justify-center !space-x-0 w-full px-3 py-2"
              >
                <div class="flex items-start justify-between w-full">
                  <span class="inline-flex items-center space-x-1 mt-0.5">
                    <UIcon :name="category.icon" class="inline text-sm opacity-40 h-4 w-4" />
                    <span>{{ category.label }}</span>
                    <tooltip v-if="category.label === 'Performance'" class="text-left">
                      <UIcon name="i-carbon-warning" class="inline text-xs mx-1" />
                      <template #tooltip>
                        <div class="mb-2">Lighthouse is running with variability. Performance scores should not be considered accurate.</div>
                        <div>Unlighthouse is running <span class="underline">with{{ throttle ? '' : 'out' }} throttling</span> which will also effect scores.</div>
                      </template>
                    </tooltip>
                  </span>
                  <div v-if="shouldShowCategoryScore(category, key)" class="flex flex-col items-end">
                    <metric-guage :score="store.categoryScores[key - 1]" :stripped="true" class="dark:font-bold" :class="store.activeTab === key ? ['dark:bg-teal-900 bg-blue-100 rounded px-2'] : []" />
                    <div
                      v-if="store.categoryScoreStats?.[category.label.toLowerCase().replace(/\s+/g, '-')]"
                      class="flex flex-col items-end text-[9px] opacity-60 mt-1 font-mono tracking-tight leading-normal"
                      :class="store.activeTab === key ? 'text-blue-200' : 'text-blue-900/60 dark:text-blue-200/60'"
                    >
                      <span>min: {{ store.categoryScoreStats[category.label.toLowerCase().replace(/\s+/g, '-')].min }}</span>
                      <span>max: {{ store.categoryScoreStats[category.label.toLowerCase().replace(/\s+/g, '-')].max }}</span>
                      <span>med: {{ store.categoryScoreStats[category.label.toLowerCase().replace(/\s+/g, '-')].median }}</span>
                    </div>
                  </div>
                </div>
              </btn-tab>
            </div>
            <div v-if="store.scanMeta?.dynamicSampling" class="text-sm opacity-70 mt-3">
              <p>Dynamically sampling is enabled, not all pages are being scanned.</p>
              <p><a href="https://unlighthouse.dev/guide/guides/dynamic-sampling" target="_blank" class="underline inline-block p-2 -m-2">Learn more about dynamic sampling</a></p>
            </div>
          </div>
          <div class="hidden xl:block">
            <div v-if="!isStatic" class="min-h-[228px]">
              <LighthouseThreeD class="mb-7" />
            </div>
            <div class="px-2 text-center xl:text-left">
              <div class="text-xs opacity-75 xl:mt-4">
                <a href="https://unlighthouse.dev" target="_blank" class="underline hover:no-underline inline-block p-1 -m-1">Documentation</a>
                <btn-action v-if="!isStatic" class="underline hover:no-underline ml-3" @click="store.toggleDebugModal">
                  Debug
                </btn-action>
              </div>
              <div class="text-xs opacity-75 xl:mt-4">
                Made with <UIcon name="i-simple-line-icons-heart" title="Love" class="inline" aria-label="love" /> by <a href="https://twitter.com/harlan_zw" target="_blank" class="underline hover:no-underline inline-block p-1 -m-1">@harlan_zw</a>
              </div>
              <div class="text-xs opacity-50 xl:mt-4 mt-1">
                Portions of this report use Lighthouse. For more information visit the <a href="https://developers.google.com/web/tools/lighthouse" class="underline hover:no-underline inline-block p-1 -m-1">Lighthouse documentation</a>.
              </div>
            </div>
          </div>
        </div>
        <div class="xl:w-full px-3 mr-5" style="view-transition-name: main-content;">
          <div v-if="filteredTabs[store.activeTab]?.label === 'CrUX'">
            <div>
              <h2 class="font-bold text-2xl mb-7">
                Origin CrUX History - Mobile
              </h2>
            </div>
            <div v-if="!store.crux && !store.cruxError" class="w-full">
              <div class="text-gray-500 text-center w-full text-sm">
                Loading CrUX data...
              </div>
            </div>
            <div v-else-if="store.cruxError" class="w-full">
              <div class="flex items-center justify-center space-x-3 p-4 bg-red-50 dark:bg-red-900/20 rounded-lg border border-red-200 dark:border-red-800">
                <UIcon name="i-carbon-warning" class="text-red-600 dark:text-red-400 text-xl" />
                <div class="text-center">
                  <p class="font-medium text-red-800 dark:text-red-200">
                    Failed to Load CrUX Data
                  </p>
                  <p class="text-sm text-red-700 dark:text-red-300 mt-1">
                    Unlighthouse CrUX API is currently unavailable.
                  </p>
                </div>
              </div>
            </div>
            <div v-else-if="store.crux?.exists === false" class="w-full">
              <div class="text-gray-500 text-center inline text-sm">
                No data from Chrome UX report
              </div>
            </div>
            <div v-else class="w-full flex-col flex space-y-5">
              <!-- CrUX Graphs ... -->
            </div>
          </div>
          <template v-else-if="!store.shouldShowWaitingState">
            <!-- Scrollable Table Container -->
            <div class="w-full overflow-x-auto pb-4">
              <div class="min-w-[1500px]">
                <!-- Table Header -->
                <div class="pr-10 pb-1 w-full">
                  <div class="grid grid-cols-12 gap-4 text-sm dark:text-gray-300 text-gray-700">
                    <results-table-head
                      v-for="(column, key) in store.resultColumns"
                      :key="key"
                      :sorting="store.sorting"
                      :column="column"
                      @sort="store.incrementSort"
                    />
                  </div>
                </div>
                <!-- Table Body -->
                <div class="w-full pr-3 overflow-y-auto xl:max-h-[calc(100vh-160px)] lg:max-h-[calc(100vh-265px)] sm:max-h-[calc(100vh-280px)] max-h-[calc(100vh-310px)]">
                  <div v-if="Object.values(store.searchResults).length === 0" class="px-4 py-3">
                    <template v-if="store.searchText">
                      <p class="mb-2">
                        No results for search "{{ store.searchText }}"...
                      </p>
                      <btn-action class="dark:bg-teal-700 bg-blue-100 px-2 text-sm" @click="store.searchText = ''">
                        Reset Search
                      </btn-action>
                    </template>
                    <template v-else-if="store.isOffline && !isStatic">
                      <div class="flex items-center space-x-3 p-4 bg-yellow-50 dark:bg-yellow-900/20 rounded-lg border border-yellow-200 dark:border-yellow-800">
                        <UIcon name="i-carbon-warning-alt" class="text-yellow-600 dark:text-yellow-400 text-xl" />
                        <div>
                          <p class="font-medium text-yellow-800 dark:text-yellow-200">
                            Server Connection Lost
                          </p>
                          <p class="text-sm text-yellow-700 dark:text-yellow-300 mt-1">
                            The Unlighthouse client is running but cannot connect to the server.
                            Please ensure the Unlighthouse server is running and accessible.
                          </p>
                        </div>
                      </div>
                    </template>
                    <template v-else-if="isStatic && (!window.__unlighthouse_payload?.reports || window.__unlighthouse_payload.reports.length === 0)">
                      <div class="flex items-center space-x-3 p-4 bg-blue-50 dark:bg-blue-900/20 rounded-lg border border-blue-200 dark:border-blue-800">
                        <UIcon name="i-carbon-information" class="text-blue-600 dark:text-blue-400 text-xl" />
                        <div>
                          <p class="font-medium text-blue-800 dark:text-blue-200">
                            No Report Data
                          </p>
                          <p class="text-sm text-blue-700 dark:text-blue-300 mt-1">
                            This is a static client build with no report data.
                            Generate reports using the Unlighthouse CLI to see lighthouse results here.
                          </p>
                        </div>
                      </div>
                    </template>
                    <div v-else class="flex items-center">
                      <loading-spinner class="mr-2" aria-label="Loading" />
                      <div>
                        <p aria-live="polite">
                          Waiting for routes...
                        </p>
                        <span class="text-xs opacity-50">If this hangs consider running Unlighthouse with --debug.</span>
                      </div>
                    </div>
                  </div>
                  <div v-else-if="store.searchText" class="px-4 py-3">
                    <p id="search-results-status" aria-live="polite">
                      Showing {{ Object.values(store.searchResults).flat().length }} routes for search "{{ store.searchText }}":
                    </p>
                  </div>
                  <results-route
                    v-for="(report, routeName) in store.paginatedResults"
                    :key="routeName"
                    v-memo="[report.route.url, report.report?.categories, report.tasks.runLighthouseTask]"
                    :report="report"
                  >
                    <template #actions>
                      <UDropdownMenu :items="getDropdownActions(report)" :content="{ placement: 'left' }">
                        <UButton
                          icon="i-heroicons-ellipsis-vertical"
                          size="sm"
                          color="neutral"
                          variant="ghost"
                          aria-label="Open actions menu"
                        />
                      </UDropdownMenu>
                    </template>
                  </results-route>
                </div>
              </div>
            </div>

            <!-- Sticky/Persistent Pagination Footer Panel -->
            <div v-if="store.searchResults.length > store.perPage" class="flex flex-col sm:flex-row items-center justify-between gap-4 mt-2 border-t border-gray-100 dark:border-gray-800/80 pt-4 w-full px-1">
              <div class="flex items-center space-x-4">
                <Pagination v-model="store.page" :page-count="store.perPage" :total="store.searchResults.length" />
                <div class="opacity-70 text-xs font-semibold font-mono text-gray-500 dark:text-gray-400">
                  {{ store.searchResults.length }} total routes
                </div>
              </div>
              <div class="hidden sm:block text-xs font-mono text-gray-400 dark:text-gray-500">
                Page {{ store.page }} of {{ Math.ceil(store.searchResults.length / store.perPage) }}
              </div>
            </div>
          </template>
          <template v-else>
            <!-- Waiting state ... -->
          </template>
        </div>
      </main>
      <footer class="block xl:hidden my-2">
        <!-- Footer ... -->
      </footer>
      <!-- Modals ... -->
      <UModal v-model:open="store.isDebugModalOpen" title="Debug Information" :ui="{ content: '!max-w-xl' }">
        <template #body>
          <div class="p-6 text-sm flex flex-col gap-6">
            <!-- App Details -->
            <div class="flex items-center justify-between border-b border-gray-200 dark:border-gray-700 pb-3">
              <div class="flex flex-col">
                <span class="font-semibold text-lg text-teal-600 dark:text-teal-400">Unlighthouse-RS</span>
                <span class="text-xs opacity-60">Version v{{ payload?.version || '0.1.0' }}</span>
              </div>
              <div class="flex items-center gap-2">
                <span class="h-2 w-2 rounded-full" :class="store.isOffline ? 'bg-red-500 animate-pulse' : 'bg-green-500'" />
                <span class="text-xs font-mono uppercase font-bold tracking-wider" :class="store.isOffline ? 'text-red-500' : 'text-green-500'">
                  {{ store.isOffline ? 'Offline' : 'Connected' }}
                </span>
              </div>
            </div>

            <!-- Grid Stats -->
            <div class="grid grid-cols-2 gap-4">
              <div class="p-3 bg-gray-50 dark:bg-gray-800/50 rounded-lg border border-gray-200 dark:border-gray-700/50">
                <div class="text-xs opacity-50 uppercase tracking-wide">
                  Target Website
                </div>
                <div class="font-medium mt-1 truncate" :title="website">
                  {{ website }}
                </div>
              </div>
              <div class="p-3 bg-gray-50 dark:bg-gray-800/50 rounded-lg border border-gray-200 dark:border-gray-700/50">
                <div class="text-xs opacity-50 uppercase tracking-wide">
                  Loaded Reports
                </div>
                <div class="font-medium mt-1 text-lg">
                  {{ store.unlighthouseReports?.length || 0 }}
                </div>
              </div>
            </div>

            <!-- Connection Stats -->
            <div class="flex flex-col gap-3">
              <h3 class="font-semibold text-gray-800 dark:text-gray-200 border-b border-gray-100 dark:border-gray-800 pb-1">
                Connection Details
              </h3>
              <div class="flex flex-col gap-2 font-mono text-xs">
                <div class="flex justify-between items-center bg-gray-50 dark:bg-gray-900/40 p-2 rounded">
                  <span class="opacity-60">API URL</span>
                  <span class="select-all">{{ apiUrl }}</span>
                </div>
                <div class="flex justify-between items-center bg-gray-50 dark:bg-gray-900/40 p-2 rounded">
                  <span class="opacity-60">WebSocket URL</span>
                  <span class="select-all">{{ payload?.options?.websocketUrl || 'N/A' }}</span>
                </div>
                <div class="flex justify-between items-center bg-gray-50 dark:bg-gray-900/40 p-2 rounded">
                  <span class="opacity-60">Static Build</span>
                  <span>{{ isStatic ? 'Yes' : 'No' }}</span>
                </div>
              </div>
            </div>

            <!-- Worker Details (if active/dynamic) -->
            <div v-if="store.scanMeta?.monitor" class="flex flex-col gap-3">
              <h3 class="font-semibold text-gray-800 dark:text-gray-200 border-b border-gray-100 dark:border-gray-800 pb-1">
                Scanner Monitor
              </h3>
              <div class="grid grid-cols-2 gap-2 text-xs">
                <div class="flex justify-between p-2 bg-gray-50 dark:bg-gray-900/40 rounded">
                  <span class="opacity-60">Status</span>
                  <span class="font-semibold capitalize text-teal-600 dark:text-teal-400">{{ store.scanMeta.monitor.status }}</span>
                </div>
                <div class="flex justify-between p-2 bg-gray-50 dark:bg-gray-900/40 rounded">
                  <span class="opacity-60">Concurrency</span>
                  <span>{{ store.scanMeta.monitor.workers }} workers</span>
                </div>
                <div class="flex justify-between p-2 bg-gray-50 dark:bg-gray-900/40 rounded">
                  <span class="opacity-60">Progress</span>
                  <span>{{ store.scanMeta.monitor.doneTargets }} / {{ store.scanMeta.monitor.allTargets }} ({{ store.scanMeta.monitor.donePercStr }}%)</span>
                </div>
                <div class="flex justify-between p-2 bg-gray-50 dark:bg-gray-900/40 rounded">
                  <span class="opacity-60">Pages / Second</span>
                  <span>{{ store.scanMeta.monitor.pagesPerSecond }}</span>
                </div>
              </div>
            </div>

            <!-- Raw Payload Explorer -->
            <div class="flex flex-col gap-2">
              <details class="group border border-gray-200 dark:border-gray-700 rounded-lg overflow-hidden">
                <summary class="flex justify-between items-center p-3 bg-gray-50 dark:bg-gray-800/80 cursor-pointer select-none font-medium hover:bg-gray-100 dark:hover:bg-gray-700/50 transition-colors">
                  <span>Raw Configuration Payload</span>
                  <UIcon name="i-heroicons-chevron-down" class="h-4 w-4 transform group-open:rotate-180 transition-transform" />
                </summary>
                <div class="p-3 bg-white dark:bg-gray-900 border-t border-gray-200 dark:border-gray-700">
                  <pre class="text-[10px] leading-relaxed font-mono p-3 bg-gray-50 dark:bg-gray-950 rounded max-h-[250px] overflow-y-auto overflow-x-auto select-all">{{ JSON.stringify(payload, null, 2) }}</pre>
                </div>
              </details>
            </div>
          </div>
        </template>
      </UModal>

      <UModal
        v-model:open="store.lighthouseReportModalOpen" title="Lighthouse Report" :ui="{
          content: '!max-w-5xl',
        }"
      >
        <template #body>
          <iframe v-if="store.iframeModalUrl" :src="store.iframeModalUrl" class="w-full h-[85vh] bg-white" />
        </template>
      </UModal>

      <UModal v-model:open="store.contentModalOpen">
        <template #body>
          <div id="modal-portal" />
        </template>
      </UModal>

      <ModalThumbnails
        v-model:open="store.thumbnailsModalOpen"
        :screenshots="store.activeScreenshots"
        @close="store.closeThumbnailsModal"
      />
    </div>
  </UApp>
</template>
