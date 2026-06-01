import { defineStore } from 'pinia'
import { ref } from 'vue'
import {
  activeScreenshots,
  activeTab,
  categoryScores,
  categoryScoreStats,
  closeAllModals,
  closeThumbnailsModal,
  contentModalOpen,
  fetchedScanMeta,
  iframeModalUrl,
  incrementSort,
  isDebugModalOpen,
  isModalOpen,
  isOffline,
  lastScanMeta,
  lighthouseReportModalOpen,
  openLighthouseReportIframeModal,
  openThumbnailsModal,
  page,
  paginatedResults,
  perPage,
  resolveArtifactPath,
  resultColumns,
  scanMeta,
  searchResults,
  searchText,
  shouldShowWaitingState,
  sorting,
  thumbnailsModalOpen,
  unlighthouseReports,
  wsReports,
} from '../logic'
import { apiUrl, isStatic, website } from '../logic/static'

export interface Sorting {
  key?: string
  dir?: 'asc' | 'desc'
}

export const useUnlighthouseStore = defineStore('unlighthouse', () => {
  // CrUX state
  const crux = ref<any>(null)
  const cruxError = ref(false)

  async function fetchCrux() {
    if (isStatic)
      return

    try {
      const response = await fetch(`${apiUrl}/crux/${encodeURIComponent(website)}/history`)
      if (!response.ok)
        throw new Error('CrUX API error')
      crux.value = await response.json()
      cruxError.value = false
    }
    catch (error) {
      console.warn('Failed to fetch CrUX data:', error)
      cruxError.value = true
    }
  }

  function toggleDebugModal() {
    isDebugModalOpen.value = !isDebugModalOpen.value
  }

  function setWsReports(reports: any[]) {
    reports.forEach((r) => {
      if (r?.route?.path)
        wsReports.set(r.route.path, r)
    })
  }

  return {
    // Shared State from logic
    activeTab,
    isDebugModalOpen,
    lighthouseReportModalOpen,
    contentModalOpen,
    thumbnailsModalOpen,
    iframeModalUrl,
    activeScreenshots,
    wsReports,
    lastScanMeta,
    fetchedScanMeta,
    isOffline,
    shouldShowWaitingState,
    categoryScores,
    categoryScoreStats,
    resultColumns,
    searchText,
    sorting,
    page,
    perPage,
    searchResults,
    paginatedResults,
    incrementSort,
    closeAllModals,
    openLighthouseReportIframeModal,
    openThumbnailsModal,
    closeThumbnailsModal,
    isModalOpen,
    unlighthouseReports,
    scanMeta,
    resolveArtifactPath,
    // Store local state & actions
    toggleDebugModal,
    setWsReports,
    crux,
    cruxError,
    fetchCrux,
  }
})
