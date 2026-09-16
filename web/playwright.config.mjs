import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {defineConfig} from '@playwright/test';

const root = path.dirname(fileURLToPath(import.meta.url));
const defaultConfig = path.resolve(root, '../../../cursor/private/web-config.lab.json');

export default defineConfig({
  testDir: './e2e',
  timeout: 120_000,
  expect: {timeout: 30_000},
  use: {
    baseURL: 'http://127.0.0.1:4175',
    trace: 'on-first-retry',
  },
  webServer: {
    command: `PODMESH_WEB_CONFIG=${process.env.PODMESH_WEB_CONFIG || defaultConfig} npm start`,
    url: 'http://127.0.0.1:4175',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
