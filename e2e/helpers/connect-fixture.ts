import { test as base, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { access, mkdtemp, mkdir, readFile, rm } from 'node:fs/promises';
import { createWriteStream } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

type ConnectFixture = { startAgent(): Promise<void>; stopAgent(): Promise<void> };
const serverPort = 3441;
const agentPort = 3442;

async function requireFreePort(port: number) {
  await new Promise<void>((resolvePort, reject) => {
    const probe = createServer();
    probe.once('error', reject);
    // Same wildcard binding used by the Rust processes. Never reuse/kill a
    // pre-existing server, even if it appears to be a previous fixture.
    probe.listen(port, '0.0.0.0', () => probe.close(error => error ? reject(error) : resolvePort()));
  });
}

async function stop(child: ChildProcess | undefined) {
  if (!child?.pid || child.exitCode !== null || child.signalCode !== null) return;
  const done = new Promise<void>(resolveExit => child.once('exit', () => resolveExit()));
  const signalGroup = (signal: NodeJS.Signals) => {
    try { process.kill(-child.pid!, signal); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error; }
  };
  // PTY shells start a new session/process group. Kill descendants while the
  // agent is alive, including on assertion failure, rather than orphaning them.
  const stopDescendants = async (pid: number) => {
    let descendants: string;
    try { descendants = await readFile(`/proc/${pid}/task/${pid}/children`, 'utf8'); }
    catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return; throw error; }
    for (const id of descendants.trim().split(/\s+/).filter(Boolean)) {
      const descendant = Number(id);
      await stopDescendants(descendant);
      try { process.kill(descendant, 'SIGKILL'); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error; }
    }
  };
  await stopDescendants(child.pid!);
  signalGroup('SIGTERM');
  const timer = setTimeout(() => signalGroup('SIGKILL'), 5000);
  try { await done; } finally { clearTimeout(timer); }
}

async function ready(child: ChildProcess, url: string) {
  await expect.poll(async () => {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`Fixture process exited before readiness: ${child.exitCode ?? child.signalCode}`);
    }
    try { return (await fetch(url, { signal: AbortSignal.timeout(1000) })).status; }
    catch { return 0; }
  }, { timeout: 30_000 }).toBe(200);
}

export const test = base.extend<{ connect: ConnectFixture }>({
  connect: async ({}, use, testInfo) => {
    if (process.platform !== 'linux') throw new Error('Connect acceptance requires the Linux real-PTY agent');
    const binary = (name: string) => {
      const value = process.env[name];
      if (!value) throw new Error(`Set ${name} to the freshly built PR binary/assets; this fixture never builds implicitly`);
      return resolve(value);
    };
    const serverBinary = binary('CONNECT_SERVER_BINARY');
    const agentBinary = binary('CONNECT_AGENT_BINARY');
    const dist = binary('CONNECT_DIST_DIR');
    await Promise.all([access(serverBinary), access(agentBinary), access(join(dist, 'index.html'))]);
    await Promise.all([requireFreePort(serverPort), requireFreePort(agentPort)]);
    const directory = await mkdtemp(join(tmpdir(), 'trakkt-connect-e2e-'));
    const configDir = join(directory, 'config');
    await mkdir(configDir);
    // Allowlist rather than process.env: inherited credentials and .env files
    // cannot select a live DB, Redis, agent config, or attachment store.
    const environment: NodeJS.ProcessEnv = {
      PATH: '/usr/bin:/bin', LANG: 'C.UTF-8', XDG_CONFIG_HOME: configDir,
      RUST_LOG: 'info',
    };
    const children: ChildProcess[] = [];
    const streams: ReturnType<typeof createWriteStream>[] = [];
    const launch = (executable: string, label: string, env: NodeJS.ProcessEnv) => {
      const log = createWriteStream(join(directory, `${label}.log`));
      streams.push(log);
      const child = spawn(executable, [], {
        cwd: directory, env: { ...environment, ...env }, detached: true,
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      child.stdout!.pipe(log, { end: false });
      child.stderr!.pipe(log, { end: false });
      children.push(child);
      return child;
    };
    let agent: ChildProcess | undefined;
    try {
      const server = launch(serverBinary, 'server', {
        TRAKKT_MODE: 'personal', PORT: String(serverPort),
        BASE_URL: `http://localhost:${serverPort}`, FRONTEND_URL: `http://localhost:${serverPort}`,
        DATABASE_URL: `sqlite://${join(directory, 'fixture.db')}?mode=rwc`,
        JWT_SECRET_KEY: 'connect-e2e-fake-development-secret',
        ENCRYPTION_KEY: Buffer.alloc(32).toString('base64'),
        TRUNK_DIST_DIR: dist, ATTACHMENT_LOCAL_PATH: join(directory, 'attachments'),
      });
      await new Promise<void>((done, reject) => {
        server.once('spawn', done);
        server.once('error', reject);
      });
      await ready(server, `http://localhost:${serverPort}/health`);
      await use({
        async startAgent() {
          if (agent) throw new Error('Agent already started');
          agent = launch(agentBinary, 'agent', {
            TRAKKT_TOKEN: 'connect-e2e-fake-token',
            TRAKKT_SERVER_URL: `ws://localhost:${serverPort}/ws/connect/agent`,
            TRAKKT_WORKING_DIR: directory, TRAKKT_ALLOWED_COMMANDS: 'bash,sh',
            TRAKKT_HEALTH_PORT: String(agentPort),
          });
          await new Promise<void>((done, reject) => {
            agent!.once('spawn', done);
            agent!.once('error', reject);
          });
          await ready(agent, `http://localhost:${agentPort}/healthz`);
        },
        async stopAgent() { await stop(agent); agent = undefined; },
      });
    } finally {
      for (const child of children.reverse()) await stop(child);
      await Promise.all(streams.map(stream => new Promise<void>(done => stream.end(done))));
      for (const label of ['server', 'agent']) {
        const log = join(directory, `${label}.log`);
        try { await testInfo.attach(`connect-${label}`, { body: await readFile(log), contentType: 'text/plain' }); }
        catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
      }
      await rm(directory, { recursive: true, force: true });
    }
  },
});
export { expect };
