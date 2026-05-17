import puppeteer from '@cloudflare/puppeteer'

interface Env {
  MY_BROWSER: any // Browser Rendering binding
  AUDIT_BUCKET: R2Bucket // R2 Bucket binding
  DB: D1Database // D1 Database binding
}

interface PerformanceVitals {
  fcp: number
  lcp: number
  cls: number
  ttfb: number
  score: number
}

// Helper to set CORS headers
const corsHeaders = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type',
}

export default {
  async fetch(request: Request, env: Env, _ctx: ExecutionContext): Promise<Response> {
    if (request.method === 'OPTIONS') {
      return new Response(null, { headers: corsHeaders })
    }

    const url = new URL(request.url)
    const targetUrl = url.searchParams.get('url')

    // 1. D1 Database Bootstrap (Schema Setup)
    if (url.pathname === '/setup') {
      try {
        await env.DB.exec(`
          CREATE TABLE IF NOT EXISTS route_reports (
            id TEXT PRIMARY KEY,
            url TEXT NOT NULL,
            path TEXT NOT NULL,
            status TEXT NOT NULL,
            score REAL,
            fcp REAL,
            lcp REAL,
            cls REAL,
            ttfb REAL,
            screenshot_key TEXT,
            updated_at TEXT NOT NULL
          )
        `)
        return new Response(JSON.stringify({ success: true, message: 'Database schema successfully created' }), {
          headers: { ...corsHeaders, 'Content-Type': 'application/json' },
        })
      }
      catch (err: any) {
        return new Response(JSON.stringify({ error: err.message }), {
          status: 500,
          headers: { ...corsHeaders, 'Content-Type': 'application/json' },
        })
      }
    }

    // 2. Fetch all saved route reports
    if (url.pathname === '/reports') {
      try {
        const { results } = await env.DB.prepare('SELECT * FROM route_reports ORDER BY path ASC').all()
        return new Response(JSON.stringify(results), {
          headers: { ...corsHeaders, 'Content-Type': 'application/json' },
        })
      }
      catch (err: any) {
        return new Response(JSON.stringify({ error: err.message }), {
          status: 500,
          headers: { ...corsHeaders, 'Content-Type': 'application/json' },
        })
      }
    }

    // 3. Main Edge Auditor Endpoint
    if (url.pathname === '/audit') {
      if (!targetUrl) {
        return new Response('Missing \'url\' query parameter', { status: 400, headers: corsHeaders })
      }

      try {
        const routePath = new URL(targetUrl).pathname
        const routeId = btoa(targetUrl).replace(/[^a-z0-9]/gi, '').slice(0, 8)

        // Update state in DB to 'Running'
        await env.DB.prepare(
          'INSERT INTO route_reports (id, url, path, status, updated_at) VALUES (?, ?, ?, \'Running\', ?) ON CONFLICT(id) DO UPDATE SET status=\'Running\', updated_at=?',
        )
          .bind(routeId, targetUrl, routePath, new Date().toISOString(), new Date().toISOString())
          .run()

        // Launch Browser Rendering instance at the Edge
        const browser = await puppeteer.launch(env.MY_BROWSER)
        const page = await browser.newPage()

        // Simulate mobile viewport & touch interface
        await page.setUserAgent('Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 Chrome/120.0.0.0 Mobile Safari/537.36')
        await page.setViewport({ width: 412, height: 823, isMobile: true, hasTouch: true })

        // Navigate to target website URL
        await page.goto(targetUrl, { waitUntil: 'load', timeout: 30000 })

        // Extract Performance metrics from browser APIs
        const vitals = await page.evaluate(() => {
          return new Promise<PerformanceVitals>((resolve) => {
            let lcp = 0
            let cls = 0
            let fcp = 0

            // Set up LCP observer
            new PerformanceObserver((entryList) => {
              const entries = entryList.getEntries()
              const lastEntry = entries[entries.length - 1]
              lcp = lastEntry.startTime
            }).observe({ type: 'largest-contentful-paint', buffered: true })

            // Set up FCP observer
            new PerformanceObserver((entryList) => {
              const entries = entryList.getEntriesByName('first-contentful-paint')
              if (entries.length > 0) {
                fcp = entries[0].startTime
              }
            }).observe({ type: 'paint', buffered: true })

            // Set up CLS observer
            new PerformanceObserver((entryList) => {
              for (const entry of entryList.getEntries()) {
                if (!(entry as any).hadRecentInput) {
                  cls += (entry as any).value
                }
              }
            }).observe({ type: 'layout-shift', buffered: true })

            // Settle and resolve all vitals after 1 second
            setTimeout(() => {
              const nav = (performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming) || {}
              const ttfb = nav.responseStart || 0

              // Compute simple composite score (0.0 to 1.0)
              let score = 100
              if (lcp > 2500)
                score -= 30
              else if (lcp > 1200)
                score -= 15

              if (fcp > 1800)
                score -= 20
              else if (fcp > 900)
                score -= 10

              if (cls > 0.25)
                score -= 30
              else if (cls > 0.1)
                score -= 15

              if (ttfb > 600)
                score -= 20
              else if (ttfb > 200)
                score -= 10

              resolve({
                fcp: Math.round(fcp),
                lcp: Math.round(lcp),
                cls: Number(cls.toFixed(3)),
                ttfb: Math.round(ttfb),
                score: Math.max(score, 0) / 100.0,
              })
            }, 1000)
          })
        })

        // Capture a gorgeous jpeg screenshot and upload directly to Cloudflare R2
        const screenshot = await page.screenshot({ fullPage: true, type: 'jpeg', quality: 80 })
        const screenshotKey = `screenshots/${routeId}.jpeg`
        await env.AUDIT_BUCKET.put(screenshotKey, screenshot, {
          httpMetadata: { contentType: 'image/jpeg' },
        })

        await browser.close()

        // Persist final metrics in SQLite (D1)
        await env.DB.prepare(
          `UPDATE route_reports 
           SET status='Completed', score=?, fcp=?, lcp=?, cls=?, ttfb=?, screenshot_key=?, updated_at=? 
           WHERE id=?`,
        )
          .bind(vitals.score, vitals.fcp, vitals.lcp, vitals.cls, vitals.ttfb, screenshotKey, new Date().toISOString(), routeId)
          .run()

        return new Response(
          JSON.stringify({
            id: routeId,
            url: targetUrl,
            status: 'Completed',
            vitals,
            screenshotKey,
          }),
          {
            headers: { ...corsHeaders, 'Content-Type': 'application/json' },
          },
        )
      }
      catch (err: any) {
        return new Response(JSON.stringify({ error: err.message }), {
          status: 500,
          headers: { ...corsHeaders, 'Content-Type': 'application/json' },
        })
      }
    }

    return new Response('Not Found', { status: 404, headers: corsHeaders })
  },
}
