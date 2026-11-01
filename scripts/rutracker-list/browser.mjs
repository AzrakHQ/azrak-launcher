// The collector's browser: the user's own Chrome, started normally with a dedicated profile and a
// debugging port, then attached to. Cloudflare's check keeps looping in a browser automation
// launched itself (automation flag, navigator.webdriver).
import { chromium } from 'playwright-core';
import { spawn, execFileSync } from 'node:child_process';

/** Launch Chrome as a normal process with a debugging port, then attach Playwright to it. */
export async function startChrome(profileDir) {
  const binary = ['google-chrome-stable', 'google-chrome', 'chromium', 'chromium-browser'].find(name => {
    try { execFileSync('which', [name], { stdio: 'ignore' }); return true; } catch { return false; }
  });
  if (!binary) throw new Error('Google Chrome or Chromium is required');
  const port = 20000 + Math.floor(Math.random() * 20000);
  const child = spawn(binary, [`--user-data-dir=${profileDir}`, `--remote-debugging-port=${port}`,
    '--remote-debugging-address=127.0.0.1', '--no-first-run', '--no-default-browser-check', 'about:blank'],
  { stdio: 'ignore' });
  for (let attempt = 0; attempt < 60; attempt++) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (response.ok) return { browser: await chromium.connectOverCDP(`http://127.0.0.1:${port}`), chrome: child };
    } catch { /* still starting */ }
    await new Promise(done => setTimeout(done, 500));
  }
  child.kill();
  throw new Error('Chrome did not start; close any Chrome window using the collector profile and retry');
}
