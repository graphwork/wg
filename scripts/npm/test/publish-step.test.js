'use strict';

// Regression guard for the first real npm publish (release run 35538576933).
//
// The operator-gated platform-publish step used to read each package name with
//   node -p "require(process.argv[1]).name" "${dir}package.json"
// A RELATIVE path handed to require() is resolved as a module specifier from
// the `[eval]` module, never from the process cwd, so the step died with
// `Cannot find module 'npm-dist/@worksgood/cli/package.json'` BEFORE the first
// npm publish. The fix reads the file explicitly and JSON-parses it.
//
// This test pins (a) that the working command form resolves a RELATIVE path and
// (b) that the release workflow keeps using that form.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const WORKFLOW = path.join(REPO_ROOT, '.github', 'workflows', 'release.yml');

const READ_NAME =
  "JSON.parse(require('fs').readFileSync(process.argv[1], 'utf8')).name";

test('publish-step name extraction resolves a RELATIVE package.json path', () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wg-publish-step-'));
  try {
    const dir = path.join(tmp, 'npm-dist', '@worksgood', 'linux-x64-gnu');
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(
      path.join(dir, 'package.json'),
      JSON.stringify({ name: '@worksgood/linux-x64-gnu' }),
    );

    const out = execFileSync(
      process.execPath,
      ['-p', READ_NAME, 'npm-dist/@worksgood/linux-x64-gnu/package.json'],
      { cwd: tmp, encoding: 'utf8' },
    );
    assert.equal(out.trim(), '@worksgood/linux-x64-gnu');
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test('release workflow platform-publish step does not require() a relative path', () => {
  const yml = fs.readFileSync(WORKFLOW, 'utf8');
  // The original defect pattern must be gone...
  assert.doesNotMatch(yml, /require\(process\.argv\[1\]\)\.name/);
  // ...and the explicit-read form must be present.
  assert.ok(
    yml.includes(READ_NAME),
    'release.yml should read package.json with fs.readFileSync + JSON.parse',
  );
});
