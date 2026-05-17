#!/usr/bin/env node
// @ts-check
/* eslint-disable no-console */
/**
 * Standalone Lighthouse worker for unlighthouse-rs.
 * Supports persistent mode for high-performance auditing.
 */

import { Buffer } from 'node:buffer'
import fs from 'node:fs'
import path from 'node:path'
import readline from 'node:readline'
import { parseArgs } from 'node:util'

/** Strip the data-URI prefix and return a Buffer. */
export function dataUriToBuffer(dataUri) {
  if (!dataUri)
    return null
  const base64 = dataUri.replace(/^data:[^;]+;base64,/, '')
  return Buffer.from(base64, 'base64')
}

// ── Globals ──────────────────────────────────────────────────────────────────
let lighthouse, chromeLauncher, chrome

async function init() {
  try {
    const lh = await import('lighthouse/core/index.cjs')
    lighthouse = lh.default ?? lh
    const cl = await import('chrome-launcher')
    chromeLauncher = cl.default ?? cl
  }
  catch (e) {
    console.error(`JSON_ERROR:${JSON.stringify({ error: `Initialization failed: ${e.message}` })}`)
    process.exit(1)
  }
}

async function injectChromeConfig(port, task) {
  const { url, userAgent, extraHeaders, auth, cookies, localStorage: ls, sessionStorage: ss } = task
  let client
  try {
    const { default: CRI } = await import('chrome-remote-interface')
    client = await CRI({ port })
    const { Page, Network } = client
    await Promise.all([Page.enable(), Network.enable()])

    // 1. User Agent Override
    if (userAgent) {
      await Network.setUserAgentOverride({ userAgent })
    }

    // 2. Extra HTTP Headers & Basic Auth
    const headers = {}
    if (extraHeaders) {
      Object.assign(headers, extraHeaders)
    }
    if (auth?.username && auth?.password) {
      const authStr = `${auth.username}:${auth.password}`
      const base64Auth = Buffer.from(authStr).toString('base64')
      headers.Authorization = `Basic ${base64Auth}`
    }
    if (Object.keys(headers).length > 0) {
      await Network.setExtraHTTPHeaders({ headers })
    }

    // 3. Cookies
    if (cookies?.length > 0) {
      const parsedUrl = new URL(url)
      const cdpCookies = cookies.map(c => ({
        name: c.name,
        value: c.value,
        domain: c.domain || parsedUrl.hostname,
        path: c.path || '/',
      }))
      await Network.setCookies({ cookies: cdpCookies })
    }

    // 4. LocalStorage & SessionStorage via addScriptToEvaluateOnNewDocument
    if (ls || ss) {
      const source = `
        localStorage.clear();
        const ls = ${JSON.stringify(ls || {})};
        for (const k in ls) localStorage.setItem(k, typeof ls[k] === 'string' ? ls[k] : JSON.stringify(ls[k]));
        sessionStorage.clear();
        const ss = ${JSON.stringify(ss || {})};
        for (const k in ss) sessionStorage.setItem(k, typeof ss[k] === 'string' ? ss[k] : JSON.stringify(ss[k]));
      `
      await Page.addScriptToEvaluateOnNewDocument({ source })
    }
  }
  catch (e) {
    console.error(`CDP injection failed: ${e.message}`)
  }
  finally {
    if (client) {
      try {
        await client.close()
      }
      catch {}
    }
  }
}

async function audit(task) {
  const { url, outputDir, device = 'mobile', throttle = false, skipJavascript = false, blockAssets = false, warmup = false } = task
  const onlyCategories = ['performance', 'accessibility', 'best-practices', 'seo']
  const formFactor = device === 'desktop' ? 'desktop' : 'mobile'

  if (!chrome) {
    chrome = await chromeLauncher.launch({
      chromeFlags: ['--headless', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage'],
    })
  }

  try {
    fs.mkdirSync(outputDir, { recursive: true })

    const flags = {
      port: chrome.port,
      output: ['json', 'html'],
      logLevel: 'error',
      onlyCategories,
      formFactor,
      screenEmulation: formFactor === 'desktop'
        ? { mobile: false, width: 1350, height: 940, deviceScaleFactor: 1, disabled: false }
        : { mobile: true, width: 375, height: 812, deviceScaleFactor: 3, disabled: false },
      throttlingMethod: throttle ? 'simulate' : 'provided',
      throttling: throttle
        ? undefined
        : { rttMs: 0, throughputKbps: 0, cpuSlowdownMultiplier: 1, requestLatencyMs: 0, downloadThroughputKbps: 0, uploadThroughputKbps: 0 },
      disableJavascript: skipJavascript,
      blockedUrlPatterns: blockAssets
        ? ['*.png', '*.jpg', '*.jpeg', '*.gif', '*.svg', '*.woff', '*.woff2', '*.ttf']
        : undefined,
    }

    // Always inject custom Chrome configs (auth, cookies, storage, userAgent, headers) before navigation
    await injectChromeConfig(chrome.port, task)

    if (warmup) {
      try {
        const { default: CRI } = await import('chrome-remote-interface')
        const client = await CRI({ port: chrome.port })
        const { Page } = client
        await Page.enable()
        await Page.navigate({ url })
        await Page.loadEventFired()
        await client.close()
      }
      catch {
        // non-fatal
      }
    }

    const result = await lighthouse(url, flags)
    if (!result?.lhr)
      throw new Error('Lighthouse returned no result')

    const lhr = result.lhr
    fs.writeFileSync(path.join(outputDir, 'report.json'), result.report[0])
    fs.writeFileSync(path.join(outputDir, 'lighthouse.html'), result.report[1])

    // Save screenshots
    const finalScreenshot = lhr.audits?.['final-screenshot']?.details?.data
    if (finalScreenshot) {
      const buf = dataUriToBuffer(finalScreenshot)
      if (buf)
        fs.writeFileSync(path.join(outputDir, 'screenshot.jpeg'), buf)
    }
    const fullPageData = lhr.fullPageScreenshot?.screenshot?.data
    if (fullPageData) {
      const buf = dataUriToBuffer(fullPageData)
      if (buf)
        fs.writeFileSync(path.join(outputDir, 'full-screenshot.jpeg'), buf)
    }

    return { success: true, url, scores: Object.fromEntries(Object.entries(lhr.categories).map(([k, v]) => [k, v.score])) }
  }
  catch (err) {
    return { success: false, url, error: err.message }
  }
}

async function run() {
  const { values } = parseArgs({
    options: {
      'url': { type: 'string' },
      'output-dir': { type: 'string' },
      'device': { type: 'string', default: 'mobile' },
      'throttle': { type: 'boolean', default: false },
      'skip-javascript': { type: 'boolean', default: false },
      'block-assets': { type: 'boolean', default: false },
      'warmup': { type: 'boolean', default: false },
      'persistent': { type: 'boolean', default: false },
    },
    strict: false,
  })

  await init()

  if (values.persistent) {
    const rl = readline.createInterface({ input: process.stdin })
    for await (const line of rl) {
      if (!line.trim())
        continue
      try {
        const task = JSON.parse(line)
        const result = await audit(task)
        console.log(`JSON_RESULT:${JSON.stringify(result)}`)
      }
      catch (e) {
        console.log(`JSON_RESULT:${JSON.stringify({ success: false, error: `Invalid task JSON: ${e.message}` })}`)
      }
    }
    if (chrome)
      await chrome.kill()
  }
  else {
    // One-off mode (backward compatibility)
    const result = await audit({
      url: values.url,
      outputDir: values['output-dir'],
      device: values.device,
      throttle: values.throttle,
      skipJavascript: values['skip-javascript'],
      blockAssets: values['block-assets'],
      warmup: values.warmup,
    })
    if (result.success) {
      const scores = Object.entries(result.scores).map(([k, v]) => `${k}: ${Math.round(v * 100)}`).join(', ')
      console.log(`✓ ${result.url} — ${scores}`)
    }
    else {
      console.error(`✗ ${result.url} — ${result.error}`)
      process.exit(1)
    }
    if (chrome)
      await chrome.kill()
  }
}

run()
