#!/usr/bin/env node
/**
 * Standalone Lighthouse worker for unlighthouse-rs.
 *
 * Usage:
 *   node lighthouse.mjs --url <url> --output-dir <path> [--device mobile|desktop] [--throttle]
 *
 * Writes into <output-dir>:
 *   report.json         — Lighthouse JSON (parsed by the Rust worker)
 *   lighthouse.html     — Lighthouse HTML report (opened in the popup iframe)
 *   screenshot.jpeg     — Final-state page screenshot (shown in route name cell)
 *   full-screenshot.jpeg — Full-page screenshot (shown in modal)
 */

import fs from 'node:fs'
import path from 'node:path'
import { parseArgs } from 'node:util'

// ── Parse CLI args ────────────────────────────────────────────────────────────

const { values } = parseArgs({
  options: {
    url:          { type: 'string' },
    'output-dir': { type: 'string' },
    device:       { type: 'string',  default: 'mobile' },
    throttle:     { type: 'boolean', default: false },
    categories:   { type: 'string',  default: 'performance,accessibility,best-practices,seo' },
  },
  strict: false,
})

const url       = values['url']
const outputDir = values['output-dir']

if (!url || !outputDir) {
  console.error('Usage: node lighthouse.mjs --url <url> --output-dir <path>')
  process.exit(1)
}

// ── Import dependencies ───────────────────────────────────────────────────────

let lighthouse, chromeLauncher

try {
  const lh = await import('lighthouse/core/index.cjs')
  lighthouse = lh.default ?? lh
} catch (e) {
  console.error('Could not import lighthouse:', e.message)
  process.exit(1)
}

try {
  const cl = await import('chrome-launcher')
  chromeLauncher = cl.default ?? cl
} catch (e) {
  console.error('Could not import chrome-launcher:', e.message)
  process.exit(1)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/** Strip the data-URI prefix and return a Buffer. */
function dataUriToBuffer(dataUri) {
  if (!dataUri) return null
  const base64 = dataUri.replace(/^data:[^;]+;base64,/, '')
  return Buffer.from(base64, 'base64')
}

// ── Run ───────────────────────────────────────────────────────────────────────

const onlyCategories = values['categories'].split(',').map(s => s.trim())
const formFactor     = values['device'] === 'desktop' ? 'desktop' : 'mobile'

let chrome
try {
  chrome = await chromeLauncher.launch({
    chromeFlags: ['--headless', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage'],
  })

  fs.mkdirSync(outputDir, { recursive: true })

  const flags = {
    port: chrome.port,
    output: ['json', 'html'],
    logLevel: 'error',
    onlyCategories,
    formFactor,
    screenEmulation: formFactor === 'desktop'
      ? { mobile: false, width: 1350, height: 940, deviceScaleFactor: 1, disabled: false }
      : { mobile: true,  width: 375,  height: 812, deviceScaleFactor: 3, disabled: false },
    throttlingMethod: values['throttle'] ? 'simulate' : 'provided',
    throttling: values['throttle']
      ? undefined
      : { rttMs: 0, throughputKbps: 0, cpuSlowdownMultiplier: 1,
          requestLatencyMs: 0, downloadThroughputKbps: 0, uploadThroughputKbps: 0 },
  }

  const result = await lighthouse(url, flags)

  if (!result?.lhr) {
    console.error('Lighthouse returned no result')
    process.exit(1)
  }

  const lhr = result.lhr

  // ── report.json (read by Rust worker) ──────────────────────────────────────
  fs.writeFileSync(path.join(outputDir, 'report.json'), result.report[0])

  // ── lighthouse.html (opened in the iframe popup) ──────────────────────────
  fs.writeFileSync(path.join(outputDir, 'lighthouse.html'), result.report[1])

  // ── screenshot.jpeg (thumbnail in route name cell) ────────────────────────
  const finalScreenshot = lhr.audits?.['final-screenshot']?.details?.data
  if (finalScreenshot) {
    const buf = dataUriToBuffer(finalScreenshot)
    if (buf) fs.writeFileSync(path.join(outputDir, 'screenshot.jpeg'), buf)
  }

  // ── full-screenshot.jpeg (full-page modal) ────────────────────────────────
  const fullPageData = lhr.audits?.['full-page-screenshot']?.details?.screenshot?.data
  if (fullPageData) {
    const buf = dataUriToBuffer(fullPageData)
    if (buf) fs.writeFileSync(path.join(outputDir, 'full-screenshot.jpeg'), buf)
  }

  // ── Log summary ───────────────────────────────────────────────────────────
  const scores = Object.entries(lhr.categories)
    .map(([k, v]) => `${k}: ${Math.round((v.score ?? 0) * 100)}`)
    .join(', ')
  console.log(`✓ ${url} — ${scores}`)

} catch (err) {
  console.error('Lighthouse error:', err?.message ?? err)
  process.exit(1)
} finally {
  await chrome?.kill()
}
