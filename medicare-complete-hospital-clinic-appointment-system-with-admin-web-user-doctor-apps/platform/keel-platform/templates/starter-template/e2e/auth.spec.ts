import { test, expect } from '@playwright/test';

// B-28 smoke: the platform auth surface (register + login) against a running
// `kelson serve`. Unique email per run so repeats never collide with an
// existing principal; cookies flow through the per-test API request context.

const uniqueEmail = (tag: string) =>
  `e2e-${tag}-${Date.now()}-${Math.floor(Math.random() * 1e6)}@starter.test`;

const password = 'correct-horse-99';

test('register issues a session and /auth/me resolves the principal', async ({
  request,
}) => {
  const reg = await request.post('/auth/register', {
    data: { email: uniqueEmail('register'), password, tenant: 'dev' },
  });
  expect(reg.ok(), await reg.text()).toBeTruthy();
  const regBody = await reg.json();
  expect(regBody.ok).toBe(true);

  const me = await request.get('/auth/me');
  expect(me.ok()).toBeTruthy();
  const meBody = await me.json();
  expect(meBody.ok).toBe(true);
  expect(meBody.data.principal).toBeTruthy();
});

test('login rejects a bad password with the envelope error', async ({
  request,
}) => {
  const email = uniqueEmail('badlogin');
  const reg = await request.post('/auth/register', {
    data: { email, password, tenant: 'dev' },
  });
  expect(reg.ok(), await reg.text()).toBeTruthy();

  const res = await request.post('/auth/login', {
    data: { identifier: email, password: 'wrong-password', tenant: 'dev' },
  });
  expect(res.status()).toBe(401);
  const body = await res.json();
  expect(body.ok).toBe(false);
  expect(body.error.code).toBeTruthy();
});

test('login with the right password resolves the same principal', async ({
  request,
}) => {
  const email = uniqueEmail('login');
  const reg = await request.post('/auth/register', {
    data: { email, password, tenant: 'dev' },
  });
  expect(reg.ok(), await reg.text()).toBeTruthy();

  const login = await request.post('/auth/login', {
    data: { identifier: email, password, tenant: 'dev' },
  });
  expect(login.ok(), await login.text()).toBeTruthy();
  const loginBody = await login.json();
  expect(loginBody.ok).toBe(true);

  const me = await request.get('/auth/me');
  const meBody = await me.json();
  expect(meBody.ok).toBe(true);
  expect(meBody.data.principal).toBeTruthy();
});
