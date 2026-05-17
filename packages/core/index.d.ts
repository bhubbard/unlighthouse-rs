/**
 * Type stubs for @unlighthouse/core.
 *
 * The JS core has been replaced by the Rust implementation (packages/core-rs).
 * This package exists solely to satisfy `import type { … } from '@unlighthouse/core'`
 * references in the client; it ships no runtime code.
 */

export type UnlighthouseTaskStatus =
  | 'completed'
  | 'in-progress'
  | 'waiting'
  | 'failed'

export interface LighthouseAudit {
  id?: string
  title: string
  description?: string
  score?: number | null
  displayValue?: string
  details?: {
    type?: string
    items?: any[]
    [key: string]: any
  }
}

export interface LighthouseCategory {
  id: string
  title?: string
  score?: number | null
  auditRefs?: Array<{ id: string; weight: number }>
}

export interface LighthouseReport {
  score?: number | null
  categories?: LighthouseCategory[]
  /** Populated client-side for easier sort lookups */
  categoryMap?: Record<string, LighthouseCategory>
  audits?: Record<string, LighthouseAudit>
}

export interface NormalisedRoute {
  id: string
  path: string
  definition?: {
    name?: string
    [key: string]: any
  }
  [key: string]: any
}

export interface UnlighthouseRouteReport {
  route: NormalisedRoute
  tasks?: {
    runLighthouseTask?: UnlighthouseTaskStatus
    inspectHtmlTask?: UnlighthouseTaskStatus
    [key: string]: UnlighthouseTaskStatus | undefined
  }
  report?: LighthouseReport
  seo?: {
    title?: string
    description?: string
    og?: {
      image?: string
      [key: string]: any
    }
    [key: string]: any
  }
  artifactUrl?: string
  [key: string]: any
}

export interface UnlighthouseColumn {
  key: string
  label: string
  sortable?: boolean
  /** Dot-path suffix appended to key when sorting; prefix with `length:` for array-length sort */
  sortKey?: string
  component?: any
  cols?: number
  slot?: string
  classes?: string[]
  [key: string]: any
}

export interface ScanMeta {
  status?: 'queued' | 'in-progress' | 'completed' | 'failed'
  monitor?: {
    cpu?: number
    memory?: number
    [key: string]: any
  }
  progress?: {
    total?: number
    completed?: number
    [key: string]: any
  }
  [key: string]: any
}

export interface ClientOptionsPayload {
  site: string
  apiUrl: string
  websocketUrl: string
  routerPrefix: string
  lighthouseOptions?: {
    onlyCategories?: string[]
    [key: string]: any
  }
  scanner: {
    dynamicSampling?: number | false
    throttle?: boolean
    device?: 'mobile' | 'desktop'
    [key: string]: any
  }
  client: {
    columns: Record<string, UnlighthouseColumn[]>
    groupRoutesKey: string
    [key: string]: any
  }
  [key: string]: any
}
