import {test, expect} from '@playwright/test';

const HOSTNAME = process.env.PODMESH_PUBLIC_HOSTNAME || 'podmesh-lab.xavdp.pro';

test('public /ready when cloudflared publisher is active', async ({request}) => {
  const response = await request.get(`https://${HOSTNAME}/ready`, {failOnStatusCode: false});
  const body = await response.text();
  const tunnelDown =
    response.status() === 530 ||
    response.status() === 502 ||
    response.status() === 503 ||
    body.includes('Error 1033') ||
    body.includes('Cloudflare Tunnel error');

  if (tunnelDown) {
    test.skip(
      true,
      `No active connector on ${HOSTNAME} (HTTP ${response.status()}). Start publisher_start on the governor lab host, then rerun.`,
    );
  }

  expect(response.status()).toBe(200);
  const json = JSON.parse(body);
  expect(json.ready).toBe(true);
  expect(json).toHaveProperty('replica_id');
  expect(json).toHaveProperty('epoch');
});
