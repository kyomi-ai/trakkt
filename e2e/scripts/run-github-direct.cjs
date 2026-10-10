// Launch only a fresh disposable server; never reuse a development instance.
const { spawn } = require('node:child_process');
const { mkdtemp, rm, access, readFile, writeFile } = require('node:fs/promises');
const { generateKeyPairSync } = require('node:crypto');
const { createWriteStream } = require('node:fs');
const { createServer } = require('node:net');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');

async function main() {
  const binary = process.env.GITHUB_DIRECT_SERVER_BINARY;
  const dist = process.env.GITHUB_DIRECT_DIST_DIR;
  if (!binary || !dist) throw new Error('Set GITHUB_DIRECT_SERVER_BINARY and GITHUB_DIRECT_DIST_DIR to freshly built binary/assets');
  await Promise.all([access(resolve(binary)), access(join(resolve(dist), 'index.html'))]);
  const port = Number(process.env.GITHUB_DIRECT_PORT ?? '3702');
  await new Promise((done, reject) => {
    const probe = createServer();
    probe.once('error', reject);
    probe.listen(port, '0.0.0.0', () => probe.close(error => error ? reject(error) : done()));
  });
  const directory = await mkdtemp(join(tmpdir(), 'trakkt-github-direct-'));
  const database = join(directory, 'fixture.db');
  const baseURL = `http://localhost:${port}`;
  const privateKeyPath = join(directory, 'fixture-app.pem');
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048,
    privateKeyEncoding: { type: 'pkcs8', format: 'pem' }, publicKeyEncoding: { type: 'spki', format: 'pem' } });
  await writeFile(privateKeyPath, privateKey, { mode: 0o600 });
  const logPath = join(directory, 'server.log');
  const log = createWriteStream(logPath);
  const server = spawn(resolve(binary), [], {
    cwd: directory, stdio: ['ignore', 'pipe', 'pipe'],
    env: {
      PATH: '/usr/bin:/bin', LANG: 'C.UTF-8', RUST_LOG: 'info',
      TRAKKT_MODE: 'saas', PORT: String(port), BASE_URL: baseURL, FRONTEND_URL: baseURL,
      DATABASE_URL: `sqlite://${database}?mode=rwc`,
      JWT_SECRET_KEY: 'direct-e2e-fake-development-secret', ENCRYPTION_KEY: Buffer.alloc(32).toString('base64'),
      TRUNK_DIST_DIR: resolve(dist), ATTACHMENT_LOCAL_PATH: join(directory, 'attachments'),
      GITHUB_APP_ID: '4242', GITHUB_APP_NAME: 'direct-e2e-fixture', GITHUB_APP_PRIVATE_KEY_PATH: privateKeyPath,
      GITHUB_WEBHOOK_SECRET: 'direct-e2e-fake-webhook-secret', GITHUB_OAUTH_CLIENT_ID: 'direct-e2e-fake-client',
      GITHUB_OAUTH_CLIENT_SECRET: 'direct-e2e-fake-client-secret',
      GITHUB_OAUTH_CALLBACK_URL: `${baseURL}/integrations/github/oauth/callback`,
    },
  });
  server.stdout.pipe(log, { end: false });
  server.stderr.pipe(log, { end: false });
  const exited = new Promise(done => server.once('exit', done));
  try {
    const deadline = Date.now() + 30_000;
    let healthy = false;
    while (Date.now() < deadline) {
      if (server.exitCode !== null) throw new Error(`Fixture server exited: ${server.exitCode}`);
      try { healthy = (await fetch(`${baseURL}/health`, { signal: AbortSignal.timeout(1000) })).status === 200; } catch {}
      if (healthy) break;
      await new Promise(done => setTimeout(done, 250));
    }
    if (!healthy) throw new Error('Disposable server did not become healthy');
    const child = spawn(process.execPath, [require.resolve('@playwright/test/cli'), 'test', '--config', 'playwright.github-direct.config.ts'], {
      cwd: resolve(__dirname, '..'), stdio: 'inherit',
      env: { PATH: process.env.PATH, HOME: process.env.HOME, LANG: 'C.UTF-8',
        BASE_URL: baseURL, GITHUB_DIRECT_E2E_DB: database, TRAKKT_MODE: 'saas' },
    });
    process.exitCode = await new Promise((done, reject) => { child.once('error', reject); child.once('exit', code => done(code ?? 1)); });
  } catch (error) {
    process.exitCode = 1;
    throw error;
  } finally {
    if (server.exitCode === null) {
      server.kill('SIGTERM');
      const timer = setTimeout(() => server.kill('SIGKILL'), 5000);
      await exited;
      clearTimeout(timer);
    }
    await new Promise(done => log.end(done));
    if (process.exitCode) console.error(await readFile(logPath, 'utf8'));
    await rm(directory, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
