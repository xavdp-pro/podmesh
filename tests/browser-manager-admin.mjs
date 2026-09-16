// The administration page in a real browser, against the responder and the same stubs as
// check-manager-admin-origin.py: the page renders, signs in without a form post, shows what is in
// a password field through the eye, keeps the fields and shows the refusal in place, forces the
// change of the deployment password, and never navigates. Run by check-manager-admin-browser.py.
import { chromium } from '../web/node_modules/playwright/index.mjs';

const base = process.argv[2];
const checks = [];
const browser = await chromium.launch();
const context = await browser.newContext({ userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36' });
const page = await context.newPage();
const navigations = [];
const posts = [];
page.on('framenavigated', f => { if (f === page.mainFrame()) navigations.push(f.url()); });
page.on('request', r => { if (r.method() === 'POST') posts.push({ url: r.url(), type: r.headers()['content-type'] || '' }); });
const errors = [];
page.on('pageerror', e => errors.push(String(e)));

await page.goto(base + '/admin');
await page.getByRole('heading', { name: 'Sign in' }).waitFor({ timeout: 10000 });
checks.push('the page renders the sign-in view from the API');

// a wrong password: refused in place, the fields keep what was typed, no navigation
await page.locator('#login').fill('admin');
await page.locator('#password').fill('Podmesh');
await page.getByRole('button', { name: 'Sign in' }).click();
await page.getByRole('alert').filter({ hasText: 'Wrong administrator or password.' }).waitFor({ timeout: 10000 });
if (await page.locator('#login').inputValue() !== 'admin') throw new Error('the login field was emptied');
if (await page.locator('#password').inputValue() !== 'Podmesh') throw new Error('the password field was emptied');
checks.push('a wrong password is refused in place and both fields keep what was typed');

// the eye shows what is really in the field -- here, the capital that made it fail
if (await page.locator('#password').getAttribute('type') !== 'password') throw new Error('the password is shown by default');
await page.getByRole('button', { name: 'Show the password' }).click();
if (await page.locator('#password').getAttribute('type') !== 'text') throw new Error('the eye did not show the password');
checks.push('the eye shows the password as typed, the capital that made it fail included');
await page.getByRole('button', { name: 'Hide the password' }).click();
if (await page.locator('#password').getAttribute('type') !== 'password') throw new Error('the eye did not hide the password again');
checks.push('the eye hides it again');

// the right one, then the forced change
await page.locator('#password').fill('podmesh');
await page.locator('#password').press('Enter');
await page.getByRole('heading', { name: 'Change the password.' }).waitFor({ timeout: 10000 });
checks.push('Enter in the password field signs in, and the deployment account lands on the change view');
await page.locator('#current').fill('podmesh');
await page.locator('#next').fill('a-good-new-password');
await page.locator('#again').fill('a-different-password');
await page.getByRole('button', { name: 'Change it' }).click();
await page.getByRole('alert').filter({ hasText: 'The two new entries differ.' }).waitFor({ timeout: 10000 });
if (await page.locator('#next').inputValue() !== 'a-good-new-password') throw new Error('the new password field was emptied');
checks.push('two differing entries are refused in place, the fields kept');

if (navigations.length !== 1) throw new Error('the page navigated: ' + JSON.stringify(navigations));
if (posts.some(p => !p.type.startsWith('application/json') || !p.url.includes('/admin/api/'))) throw new Error('a post that is not a JSON API call: ' + JSON.stringify(posts));
if (errors.length) throw new Error('script errors: ' + errors.join(' | '));
checks.push(`no navigation after the first load, and every one of ${posts.length} posts a JSON call to the API -- no form post`);
await browser.close();
console.log(JSON.stringify({ result: 'PASS', checks }, null, 2));
