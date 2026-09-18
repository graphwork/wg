'use strict';

// Unit tests for the @worksgood/cli resolver (scripts/npm/cli/lib/resolver.js).
// Run: node --test scripts/npm/test/

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const resolver = require('../cli/lib/resolver.js');

test('platform map: linux-x64 glibc -> @worksgood/linux-x64-gnu', () => {
  assert.equal(
    resolver.platformPackageName({ platform: 'linux', arch: 'x64', linuxLibc: 'gnu' }),
    '@worksgood/linux-x64-gnu',
  );
});

test('platform map: linux-x64 musl -> separately-named musl package', () => {
  assert.equal(
    resolver.platformPackageName({ platform: 'linux', arch: 'x64', linuxLibc: 'musl' }),
    '@worksgood/linux-x64-musl',
  );
});

test('platform map: darwin-arm64 -> @worksgood/darwin-arm64', () => {
  assert.equal(
    resolver.platformPackageName({ platform: 'darwin', arch: 'arm64' }),
    '@worksgood/darwin-arm64',
  );
});

test('platform map: phase-2 targets have no package yet (null)', () => {
  assert.equal(resolver.platformPackageName({ platform: 'darwin', arch: 'x64' }), null);
  assert.equal(resolver.platformPackageName({ platform: 'win32', arch: 'x64' }), null);
  assert.equal(resolver.platformPackageName({ platform: 'linux', arch: 'arm64' }), null);
  assert.equal(resolver.platformPackageName({ platform: 'freebsd', arch: 'x64' }), null);
});

test('musl detection: glibc report header wins', () => {
  const fakeReport = { getReport: () => ({ header: { glibcVersionRuntime: '2.31' } }) };
  assert.equal(resolver.detectLinuxLibc(fakeReport), 'gnu');
});

test('musl detection: no glibcVersionRuntime -> conservative musl', () => {
  const fakeReport = { getReport: () => ({ header: {} }) };
  assert.equal(resolver.detectLinuxLibc(fakeReport), 'musl');
});

test('musl detection: a throwing report falls back to musl (fail closed to fallback hint)', () => {
  const fakeReport = {
    getReport: () => {
      throw new Error('boom');
    },
  };
  assert.equal(resolver.detectLinuxLibc(fakeReport), 'musl');
});

test('fallback message names the target and the cargo install hint', () => {
  const msg = resolver.fallbackMessage('x64-linux-musl', 'musl builds are not published yet');
  assert.match(msg, /no prebuilt binary for x64-linux-musl/);
  assert.match(msg, /musl builds are not published yet/);
  assert.match(msg, /cargo install --git https:\/\/github\.com\/graphwork\/wg --locked/);
});

test('unsupported platform run prints the fallback hint and exits 1', () => {
  let stderr = '';
  const origWrite = process.stderr.write.bind(process.stderr);
  const origExit = process.exit;
  process.stderr.write = (s) => {
    stderr += s;
    return true;
  };
  process.exit = (code) => {
    assert.equal(code, 1);
    process.exit = origExit;
    process.stderr.write = origWrite;
    throw new Error('__exit_called__');
  };
  assert.throws(
    () => resolver.run('wg', { platform: 'sunos', arch: 'x64', argv: [] }),
    /__exit_called__/,
  );
  process.stderr.write = origWrite;
  process.exit = origExit;
  assert.match(stderr, /no prebuilt binary for/);
  assert.match(stderr, /cargo install/);
});

test('WG_BINARY_PATH pointing at a missing file fails loudly with the var named', () => {
  let stderr = '';
  const origWrite = process.stderr.write.bind(process.stderr);
  const origExit = process.exit;
  process.stderr.write = (s) => {
    stderr += s;
    return true;
  };
  process.exit = (code) => {
    assert.equal(code, 1);
    process.exit = origExit;
    throw new Error('__exit_called__');
  };
  assert.throws(
    () =>
      resolver.run('wg', {
        env: { WG_BINARY_PATH: '/definitely/not/here/wg' },
        argv: [],
      }),
    /__exit_called__/,
  );
  process.stderr.write = origWrite;
  process.exit = origExit;
  assert.match(stderr, /WG_BINARY_PATH/);
  assert.match(stderr, /does not exist/);
});

test('WG_BINARY_PATH override execs the pointed-at binary and forwards the exit code', async () => {
  // Real end-to-end: a stub "binary" that exits 7; run() should exec it, not
  // resolve any platform package, and exit 7. Spawned detached from the test
  // flow via a child node process because run() calls process.exit.
  const { execFileSync } = require('node:child_process');
  const os = require('node:os');
  const fs = require('node:fs');
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wg-shim-test-'));
  const stub = path.join(tmp, 'stub-bin');
  fs.writeFileSync(stub, '#!/bin/sh\necho OVERRIDE-OK "$@"\nexit 7\n');
  fs.chmodSync(stub, 0o755);

  const runner = path.join(tmp, 'runner.js');
  fs.writeFileSync(
    runner,
    `require(${JSON.stringify(path.resolve(__dirname, '../cli/lib/resolver.js'))})
      .run('wg', { env: { WG_BINARY_PATH: ${JSON.stringify(stub)} }, argv: ['--flag', 'val'] });`,
  );

  let stdout = '';
  let code = 0;
  try {
    stdout = execFileSync(process.execPath, [runner], { encoding: 'utf8' });
  } catch (err) {
    code = err.status;
    stdout = err.stdout;
  }
  assert.match(stdout, /OVERRIDE-OK --flag val/);
  assert.equal(code, 7);
});
