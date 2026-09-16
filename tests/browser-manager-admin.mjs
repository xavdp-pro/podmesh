// The administration app in a real browser, against the Express origin and the same stubs as the
// API tests: rendered from the API, a wrong password refused in place with the fields kept, the eye
// showing then hiding what is typed, Enter signing in, the deployment account forced to the change
// view, forms as containers only, a session that ends sent back to sign-in with the reason, no
// navigation, every post a JSON call, and words when JavaScript is off. Run against a local origin by
// check-manager-admin-browser.py, or against the public hostname with its URL as the argument.
import { chromium } from '../web/node_modules/playwright/index.mjs';

const base = process.argv[2];
const checks = [];
const UA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36';
const browser = await chromium.launch();
const context = await browser.newContext({ userAgent: UA, locale: 'en-US' });
const page = await context.newPage();
const navigations = [];
const posts = [];
let dialogs = 0;
page.on('framenavigated', f => { if (f === page.mainFrame()) navigations.push(f.url()); });
page.on('request', r => { if (r.method() === 'POST') posts.push({ url: r.url(), type: r.headers()['content-type'] || '' }); });
page.on('dialog', d => { dialogs++; d.dismiss(); });
const errors = [];
page.on('pageerror', e => errors.push(String(e)));

await page.goto(base + '/admin');
await page.getByRole('heading', { name: 'Sign in' }).waitFor({ timeout: 15000 });
checks.push('the app renders the sign-in view from the API');

const login = page.getByLabel('Administrator');
const password = page.getByLabel('Password', { exact: true });
await login.fill('admin');
await password.fill('Podmesh');
await page.getByRole('button', { name: 'Sign in' }).click();
await page.getByRole('alert').filter({ hasText: 'Wrong administrator or password.' }).waitFor({ timeout: 10000 });
if (await login.inputValue() !== 'admin') throw new Error('the login field was emptied');
if (await password.inputValue() !== 'Podmesh') throw new Error('the password field was emptied');
checks.push('a wrong password is refused in place and both fields keep what was typed');

if (await password.getAttribute('type') !== 'password') throw new Error('the password is shown by default');
await page.getByRole('button', { name: 'Show the password' }).click();
if (await password.getAttribute('type') !== 'text') throw new Error('the eye did not show the password');
checks.push('the eye shows the password as typed, the capital that made it fail included');
await page.getByRole('button', { name: 'Hide the password' }).click();
if (await password.getAttribute('type') !== 'password') throw new Error('the eye did not hide the password again');
checks.push('the eye hides it again');

await password.fill('podmesh');
await password.press('Enter');
await page.getByRole('heading', { name: 'Change the password.' }).waitFor({ timeout: 10000 });
checks.push('Enter in the password field signs in, and the deployment account lands on the change view');

const current = page.getByLabel('Current password');
const next = page.getByLabel('New password', { exact: true });
const again = page.getByLabel('New password again');
await current.fill('podmesh');
await next.fill('a-good-new-password');
await again.fill('a-different-password');
await page.getByRole('button', { name: 'Change it' }).click();
await page.getByRole('alert').filter({ hasText: 'The two new entries differ.' }).waitFor({ timeout: 10000 });
if (await next.inputValue() !== 'a-good-new-password') throw new Error('the new password field was emptied');
checks.push('two differing entries are refused in place, the fields kept');

const forms = await page.evaluate(() => [...document.querySelectorAll('form')].map(f => ({ method: f.getAttribute('method'), action: f.getAttribute('action') })));
if (!forms.length || forms.some(f => f.method || f.action)) throw new Error('a form that could post natively: ' + JSON.stringify(forms));
checks.push(`the fields sit in ${forms.length} form(s) with neither method nor action, so a password manager pairs them and nothing can post natively`);

await context.clearCookies();
await again.fill('a-good-new-password');
await page.getByRole('button', { name: 'Change it' }).click();
await page.getByRole('heading', { name: 'Sign in' }).waitFor({ timeout: 10000 });
await page.getByRole('alert').filter({ hasText: 'Your session ended. Sign in again.' }).waitFor({ timeout: 10000 });
checks.push('a session that ended mid-way sends the app back to sign-in with the reason, not a refusal nobody can act on');

if (navigations.length !== 1) throw new Error('the page navigated: ' + JSON.stringify(navigations));
if (posts.some(p => !p.type.startsWith('application/json') || !p.url.includes('/admin/api/'))) throw new Error('a post that is not a JSON API call: ' + JSON.stringify(posts));
if (dialogs) throw new Error(`${dialogs} browser dialog(s) opened`);
if (errors.length) throw new Error('script errors: ' + errors.join(' | '));
checks.push(`no navigation after the first load, no browser dialog, and every one of ${posts.length} posts a JSON call to the API -- no form post`);

const bare = await browser.newContext({ javaScriptEnabled: false, userAgent: UA });
const still = await bare.newPage();
await still.goto(base + '/admin');
const words = await still.locator('body').innerText();
if (!words.includes('This page needs JavaScript')) throw new Error('no words without JavaScript: ' + words.slice(0, 200));
checks.push('with JavaScript off the page says it needs it, instead of a blank page');
await browser.close();
console.log(JSON.stringify({ result: 'PASS', checks }, null, 2));
