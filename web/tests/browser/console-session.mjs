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
const moves = [];
const make = (broken = false) => createApp({hosts: [{id: 'a', name: 'lab-a', ssh: 'lab@a', remoteSocket: '/run/x/api.sock', allowActions: true}, {id: 'b', name: 'lab-b', ssh: 'lab@b', remoteSocket: '/run/x/api.sock', allowActions: true}]}, {
  origin: url,
  runMove: async m => { moves.push(m); return {result: 'moved', message: 'the universe runs on the destination', steps: [{step: 'checkpoint', host: 'source', ok: true, seconds: 1.8}, {step: 'restore', host: 'destination', ok: true, seconds: 1.9}]}; },
  call: async (_h, p) => {
    calls.push(p);
    const data = {
      identity: {host_uuid: '00000000-0000-4000-8000-00000000000a'},
      capabilities: {version: 'fixture', operations: ['create', 'start', 'stop', 'pause', 'resume', 'resources', 'delete', 'clone', 'storage_status'], schemas: {
        stop: {kind: 'universe', gate: 'none', description: 'sends the stop signal and waits', fields: [{name: 'timeout_seconds', type: 'integer', required: true, min: 0, max: 300, description: 'the graceful wait'}, {name: 'on_timeout', type: 'enum', required: true, values: ['kill', 'leave_running'], description: 'escalate or leave running'}]},
        storage_status: {kind: 'read', gate: 'none', description: 'what carries the storage', fields: []},
        migration_checkpoint: {kind: 'tool', gate: 'reservation', description: 'a step of the chain', fields: null},
      }},
      inventory: {containers: broken || _h.id !== 'a' ? [] : [{Id: 'c'.repeat(64), Names: ['podmesh-11111111-1111-4111-8111-111111111111'], State: 'running', Image: 'localhost/fixture', Labels: {'io.podmesh.universe': '11111111-1111-4111-8111-111111111111'}}]},
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
  page.on('console', m => { if (m.type() === 'error' && m.text().includes('drawing error')) console.error('CONSOLE:', m.text().slice(0, 1200)); });
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
  const before = calls.filter(c => c.operation === 'create').length;
  await page.getByRole('button', {name: 'Confirm create'}).click();
  await page.getByText('Session renewed — retry the same request').waitFor({timeout: 15000});
  if (calls.filter(c => c.operation === 'create').length !== before) throw new Error('the stale-token request reached the runtime');
  checks.push('an action sent after the console restarted comes back as a renewed session to retry, and never reached the runtime');

  await page.getByRole('button', {name: 'Retry same request'}).click();
  try {
    await page.getByText('API success — inspect the observed result').waitFor({timeout: 15000});
  } catch (e) {
    console.error('DIALOG TEXT:', (await page.getByRole('dialog').innerText()).slice(0, 1500));
    throw e;
  }
  if (calls.filter(c => c.operation === 'create').length !== before + 1) throw new Error('the retry did not reach the runtime exactly once');
  checks.push('the retry, keeping its operation identity, succeeds with the renewed session and reaches the runtime once');

  // a running universe: pause offered, resume not; resources asks memory and cpus and sends them as bytes and cores
  await page.getByRole('button', {name: 'Close action'}).click();
  await page.getByRole('button', {name: 'podmesh-11111111-1111-4111-8111-111111111111', exact: true}).click();
  const drawer = page.getByRole('dialog', {name: 'Container details'});
  await drawer.waitFor({timeout: 10000});
  if (await drawer.getByRole('button', {name: 'pause', exact: true}).isDisabled()) throw new Error('pause is not offered on a running universe');
  if (!await drawer.getByRole('button', {name: 'resume', exact: true}).isDisabled()) throw new Error('resume is offered on a running universe');
  if (!await drawer.getByRole('button', {name: 'start', exact: true}).isDisabled()) throw new Error('start is offered on a running universe');
  checks.push('on a running universe the drawer offers pause, and neither resume nor start');
  await drawer.getByRole('button', {name: 'resources', exact: true}).click();
  const form2 = page.getByRole('dialog', {name: 'resources universe'});
  await form2.waitFor({timeout: 10000});
  await form2.getByLabel('Memory limit (MiB, at least 32)').fill('512');
  await form2.getByLabel('CPU allowance (cores, from 0.1)').fill('1.5');
  await form2.getByPlaceholder('Approved task or mandate').fill('browser-check');
  const sent = calls.length;
  await form2.getByRole('button', {name: 'Confirm resources'}).click();
  await page.getByText('API success — inspect the observed result').waitFor({timeout: 15000});
  const last = calls.slice(sent).find(c => c.operation === 'resources');
  if (!last) throw new Error('no resources call reached the runtime: ' + JSON.stringify(calls.slice(sent)));
  if (last.memory_bytes !== 512 * 1024 * 1024 || last.cpus !== 1.5 || 'memory_mib' in last) throw new Error('the resources request is not what was typed: ' + JSON.stringify(last));
  checks.push('resources typed as 512 MiB and 1.5 cores reaches the runtime as memory_bytes 536870912 and cpus 1.5, nothing else');
  await page.getByRole('button', {name: 'Close action'}).click();

  // move: the destination chosen among the other SSH hosts, the report's steps shown
  await drawer.getByRole('button', {name: 'move', exact: true}).click();
  const form3 = page.getByRole('dialog', {name: 'move universe'});
  await form3.waitFor({timeout: 10000});
  await form3.getByRole('button', {name: 'lab-b'}).click();
  await form3.getByPlaceholder('Approved task or mandate').fill('browser-check');
  await form3.getByRole('button', {name: 'Confirm move'}).click();
  await page.getByText('Moved — the universe runs on the destination').waitFor({timeout: 15000});
  await page.getByText('restore · destination · ok · 1.9s').waitFor({timeout: 5000});
  if (moves.length !== 1 || moves[0].source.id !== 'a' || moves[0].destination.id !== 'b' || moves[0].universe_uuid !== '11111111-1111-4111-8111-111111111111') throw new Error('the move request is not what was chosen: ' + JSON.stringify(moves));
  checks.push('move offers the other SSH host as destination, sends the chosen universe and hosts to the move tool, and shows the report\'s steps');
  await page.getByRole('button', {name: 'Close action'}).click();

  // the generic engine: any operation, its form drawn from the host's schema, validated, sent as JSON
  await page.getByRole('button', {name: 'Close details'}).click();
  await page.getByRole('button', {name: 'Run'}).click();
  const form4 = page.locator('section.op-form');
  try {
    await form4.waitFor({timeout: 10000});
  } catch (e) {
    console.error('PAGE ERRORS:', errors.join(' | '));
    console.error('PAGE TEXT:', (await page.locator('body').innerText()).slice(0, 800));
    throw e;
  }
  await form4.getByRole('button', {name: 'Operation'}).click();
  await page.getByLabel('Search an operation…').fill('sto');
  await page.getByRole('option', {name: /^stop/}).click();
  await form4.getByLabel('timeout_seconds').waitFor({timeout: 5000});
  checks.push('the Run view lists the host\'s operations in a searchable styled list and draws stop\'s fields from its schema');
  await form4.getByLabel('timeout_seconds').fill('900');
  await form4.getByRole('button', {name: 'on_timeout'}).click();
  await page.getByRole('option', {name: 'leave_running'}).click();
  await form4.getByRole('button', {name: 'Universe'}).click();
  await page.getByRole('option', {name: /podmesh-11111111/}).click();
  await form4.getByPlaceholder('Approved task or mandate').fill('browser-check');
  const beforeStop = calls.filter(c => c.operation === 'stop').length;
  await form4.getByRole('button', {name: 'Send stop'}).click();
  await form4.getByRole('alert').filter({hasText: 'timeout_seconds is at most 300'}).waitFor({timeout: 5000});
  if (calls.filter(c => c.operation === 'stop').length !== beforeStop) throw new Error('an out-of-bound value reached the runtime');
  checks.push('a value outside the schema\'s bound is refused in place before anything is sent');
  await form4.getByLabel('timeout_seconds').fill('15');
  await form4.getByRole('button', {name: 'Send stop'}).click();
  await page.getByText('The host answered ok').waitFor({timeout: 10000});
  const stopSent = calls.filter(c => c.operation === 'stop').slice(-1)[0];
  if (!stopSent || stopSent.timeout_seconds !== 15 || stopSent.on_timeout !== 'leave_running' || stopSent.universe_uuid !== '11111111-1111-4111-8111-111111111111' || stopSent.authorization_ref !== 'browser-check') throw new Error('the generic request is not what was typed: ' + JSON.stringify(stopSent));
  checks.push('the generic form sends stop with timeout_seconds as an integer, the chosen enum, the chosen universe and the mandate');
  await form4.getByRole('button', {name: 'Operation'}).click();
  await page.getByLabel('Search an operation…').fill('migration');
  await page.getByRole('option', {name: /migration checkpoint/}).click();
  await form4.getByText('driven by a tool from this workstation').waitFor({timeout: 5000});
  if (await form4.getByRole('button', {name: /^Send/}).count()) throw new Error('a tool step is offered for sending');
  checks.push('a step of a cross-host chain is shown as the tool\'s, with nothing to send');

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
