// The console in a real browser, against its own server code (createApp) with a fake runtime, the
// built page, and a console "restart" in the middle: the running app is replaced, so the page's token
// goes stale. Verified: the page loads; the action form posts nothing natively; an action sent with the
// stale token comes back as a renewed session the operator can retry, not a refusal; the retry, which
// keeps its operation identity, succeeds; a drawing error shows words and a Reload button; and without
// JavaScript the page says what it needs.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {chromium} from '../../node_modules/playwright/index.mjs';
import {createApp} from '../../server/app.mjs';

const web = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const dist = path.join(web, 'dist');
const calls = [];
let app;
const server = http.createServer((req, res) => {
  if (req.url.startsWith('/api/')) return app(req, res);
  const file = req.url === '/' || !req.url.startsWith('/assets/') ? path.join(dist, 'index.html') : path.join(dist, req.url);
  if (!file.startsWith(dist) || !fs.existsSync(file)) { res.writeHead(404); return res.end(); }
  res.writeHead(200, {'Content-Type': file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html'});
  fs.createReadStream(file).pipe(res);
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const url = 'http://127.0.0.1:' + server.address().port;
const make = (broken = false) => createApp({hosts: [{id: 'a', name: 'lab-a', socket: '/fixture.sock', allowActions: true}]}, {
  origin: url,
  call: async (_h, p) => {
    calls.push(p.operation);
    const data = {
      identity: {host_uuid: '00000000-0000-4000-8000-00000000000a'},
      capabilities: {version: 'fixture', operations: ['create', 'start', 'stop', 'delete', 'clone']},
      inventory: {containers: []},
      observations: {observations: broken ? 'not a list' : []},
    }[p.operation] || {};
    return {ok: true, data};
  },
});
app = make();

const checks = [];
const browser = await chromium.launch();
try {
  const context = await browser.newContext();
  const page = await context.newPage();
  const navigations = [];
  page.on('framenavigated', f => { if (f === page.mainFrame()) navigations.push(f.url()); });
  const errors = [];
  page.on('pageerror', e => errors.push(String(e)));
  let dialogs = 0;
  page.on('dialog', d => { dialogs++; d.dismiss(); });

  await page.goto(url + '/');
  await page.getByRole('button', {name: /Universes/}).first().click();
  const create = page.getByRole('button', {name: 'Create universe'}).first();
  try {
    await create.waitFor({timeout: 20000});
  } catch (e) {
    console.error('PAGE TEXT:', (await page.locator('body').innerText()).slice(0, 1500));
    throw e;
  }
  for (let i = 0; i < 40 && await create.isDisabled(); i++) await page.waitForTimeout(500);
  if (await create.isDisabled()) {
    console.error('PAGE TEXT:', (await page.locator('body').innerText()).slice(0, 1500));
    throw new Error('Create universe stays disabled');
  }
  checks.push('the console renders from its API');

  // the console restarts: same address, a new app, a new token -- the page still holds the old one
  app = make();
  await create.click();
  await page.getByRole('dialog').waitFor({timeout: 10000});
  const forms = await page.evaluate(() => [...document.querySelectorAll('form')].map(f => ({method: f.getAttribute('method'), action: f.getAttribute('action')})));
  if (!forms.length || forms.some(f => f.method || f.action)) throw new Error('a form that could post natively: ' + JSON.stringify(forms));
  checks.push('the action form has neither method nor action');
  await page.getByRole('dialog').getByRole('button', {name: 'lab-a'}).click();
  await page.getByPlaceholder('sha256:…').fill('sha256:' + 'a'.repeat(64));
  await page.getByPlaceholder('Approved task or mandate').fill('browser-check');
  const before = calls.filter(c => c === 'create').length;
  await page.getByRole('button', {name: 'Confirm create'}).click();
  await page.getByText('Session renewed — retry the same request').waitFor({timeout: 15000});
  if (calls.filter(c => c === 'create').length !== before) throw new Error('the stale-token request reached the runtime');
  checks.push('an action sent after the console restarted comes back as a renewed session to retry, and never reached the runtime');

  await page.getByRole('button', {name: 'Retry same request'}).click();
  await page.getByText('API success — inspect the observed result').waitFor({timeout: 15000});
  if (calls.filter(c => c === 'create').length !== before + 1) throw new Error('the retry did not reach the runtime exactly once');
  checks.push('the retry, keeping its operation identity, succeeds with the renewed session and reaches the runtime once');

  if (navigations.length !== 1) throw new Error('the console navigated: ' + JSON.stringify(navigations));
  if (dialogs) throw new Error(`${dialogs} browser dialog(s) opened`);
  if (errors.length) throw new Error('script errors: ' + errors.join(' | '));
  checks.push('no navigation, no browser dialog, no script error');

  // a gateway answer the console does not expect makes drawing fail: words, not a blank page
  app = make(true);
  const odd = await context.newPage();
  const oddErrors = [];
  odd.on('pageerror', e => oddErrors.push(String(e)));
  await odd.goto(url + '/');
  await odd.getByRole('heading', {name: 'The console could not draw this view.'}).waitFor({timeout: 20000});
  await odd.getByRole('button', {name: 'Reload'}).waitFor({timeout: 5000});
  checks.push('a drawing error, from a gateway answer the console does not expect, shows what happened and a Reload button instead of a blank page');

  const bare = await browser.newContext({javaScriptEnabled: false});
  const still = await bare.newPage();
  await still.goto(url + '/');
  const words = await still.locator('body').innerText();
  if (!words.includes('This console needs JavaScript')) throw new Error('no words without JavaScript: ' + words.slice(0, 200));
  checks.push('with JavaScript off the console says it needs it, instead of a blank page');
  console.log(JSON.stringify({result: 'PASS', checks}, null, 2));
} finally {
  await browser.close();
  server.close();
}
