import {test, expect} from '@playwright/test';

test('operator console loads and shows lab hosts', async ({page}) => {
  await page.goto('/');
  await expect(page.getByRole('link', {name: /PodMesh/i})).toBeVisible();
  await expect(page.getByRole('heading', {name: /Your mesh, at a glance/i})).toBeVisible();
  await expect(page.getByRole('heading', {name: 'Lab A'})).toBeVisible({timeout: 90_000});
  await expect(page.getByRole('heading', {name: 'Lab B'})).toBeVisible();
  await expect(page.getByRole('heading', {name: 'Lab C'})).toBeVisible();
});

test('Manager view states HA is not qualified', async ({page}) => {
  await page.goto('/');
  await page.getByRole('button', {name: 'Manager'}).click();
  await expect(page.getByRole('heading', {name: /One logical manager/i})).toBeVisible();
  await expect(page.getByText('Safe takeover and recovery: not qualified')).toBeVisible();
});
