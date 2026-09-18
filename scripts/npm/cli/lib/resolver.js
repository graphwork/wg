'use strict';

/*
 * Platform resolution for @worksgood/cli.
 *
 * This is the esbuild (PR #1621) / biome optionalDependencies model: the
 * metapackage ships ZERO install scripts; the package manager itself installs
 * exactly the right platform package as an optionalDependency, and this shim
 * only resolves the binary inside it and execs it.
 *
 * See docs/research/npm-distribution.md (§2 resolution/exec/fallback) and
 * scripts/npm/README.md.
 */

const { spawn } = require('node:child_process');
const { createRequire } = require('node:module');
const fs = require('node:fs');
const path = require('node:path');

const PACKAGE_SCOPE = '@worksgood';

// The documented fallback when no prebuilt binary applies. WG deliberately
// improves on esbuild here (whose legacy fallback was a nested `npm install`
// download): a clear "use cargo install" hint is simpler and safer than
// npm-inside-npm. docs/research/npm-distribution.md §2 step 5.
const CARGO_INSTALL_HINT = 'cargo install --git https://github.com/graphwork/wg --locked';

/*
 * glibc-vs-musl is not expressible in npm `os`/`cpu` fields, so musl is a
 * separately-named package (biome convention) detected at runtime. WG ships
 * glibc-only Linux today, so a musl detection result lands on the fallback
 * path until the phase-2 `*-musl` packages are published.
 */
function detectLinuxLibc(report) {
  try {
    const rep = report || process.report;
    if (rep && typeof rep.getReport === 'function') {
      const header = rep.getReport().header;
      if (header && header.glibcVersionRuntime) return 'gnu';
    }
  } catch (_) {
    // Fall through to the conservative answer below.
  }
  return 'musl';
}

/*
 * process.platform/process.arch -> platform package name. Returns null for
 * combinations with no published package (phase-2 targets: linux-arm64-gnu,
 * darwin-x64, win32-x64, and the musl pair).
 */
function platformPackageName(opts) {
  const { platform = process.platform, arch = process.arch, linuxLibc } = opts || {};
  if (platform === 'linux' && arch === 'x64') {
    const libc = linuxLibc || detectLinuxLibc();
    return libc === 'gnu' ? `${PACKAGE_SCOPE}/linux-x64-gnu` : `${PACKAGE_SCOPE}/linux-x64-musl`;
  }
  if (platform === 'darwin' && arch === 'arm64') return `${PACKAGE_SCOPE}/darwin-arm64`;
  return null;
}

/* Human-readable target label for messages ("x64-linux-gnu", "arm64-darwin"). */
function humanTarget(opts) {
  const { platform = process.platform, arch = process.arch, linuxLibc } = opts || {};
  if (platform === 'linux') {
    return `${arch === 'x64' ? 'x64' : arch}-linux-${linuxLibc || detectLinuxLibc()}`;
  }
  return `${arch}-${platform}`;
}

function fallbackMessage(target, reason) {
  const lines = [`@worksgood/cli: no prebuilt binary for ${target}.`];
  if (reason) lines.push(`(${reason})`);
  lines.push('Install the Rust toolchain build instead:', `  ${CARGO_INSTALL_HINT}`);
  return lines.join('\n');
}

/*
 * Exec the real binary with inherited stdio and forward the exit code.
 * Exported for tests; the shims reach it through run().
 */
function execBinary(binaryPath, argv) {
  const child = spawn(binaryPath, argv, { stdio: ['inherit', 'inherit', 'inherit'] });

  // Forward termination signals the shim itself receives (biome-style).
  const forward = (signal) => {
    if (!child.killed && child.exitCode === null) child.kill(signal);
  };
  process.on('SIGINT', () => forward('SIGINT'));
  process.on('SIGTERM', () => forward('SIGTERM'));

  child.on('error', (err) => {
    process.stderr.write(`@worksgood/cli: failed to exec ${binaryPath}: ${err.message}\n`);
    process.exit(1);
  });
  child.on('exit', (code, signal) => {
    if (signal) {
      // Shell convention: 128 + signal number for the common cases.
      const num = signal === 'SIGINT' ? 2 : signal === 'SIGTERM' ? 15 : 1;
      process.exit(128 + num);
    }
    process.exit(code === null ? 1 : code);
  });
}

/*
 * Entry point used by bin/wg.js, bin/nex.js and bin/worksgood.js.
 * `opts` exists for unit tests (inject platform/arch/env/argv); production
 * callers pass nothing.
 */
function run(binName, opts) {
  const o = opts || {};
  const platform = o.platform || process.platform;
  const arch = o.arch || process.arch;
  const env = o.env || process.env;
  const argv = o.argv !== undefined ? o.argv : process.argv.slice(2);

  // 1. Escape hatch: WG_BINARY_PATH (mirrors ESBUILD_BINARY_PATH / BIOME_BINARY)
  //    for distro-packaged or self-built binaries. Points at the exact binary
  //    to exec for the command being run.
  const override = env.WG_BINARY_PATH;
  if (override) {
    const resolved = path.resolve(override);
    if (!fs.existsSync(resolved)) {
      process.stderr.write(
        `@worksgood/cli: WG_BINARY_PATH is set to "${override}" but that file does not exist.\n`,
      );
      process.exit(1);
    }
    return execBinary(resolved, argv);
  }

  // 2. Resolve the platform package the package manager already installed.
  const pkg = platformPackageName({ platform, arch });
  if (!pkg) {
    const target = humanTarget({ platform, arch });
    process.stderr.write(
      fallbackMessage(target, `no @worksgood platform package for ${platform}/${arch} yet`) + '\n',
    );
    process.exit(1);
  }
  if (pkg.endsWith('-musl')) {
    const target = humanTarget({ platform, arch, linuxLibc: 'musl' });
    process.stderr.write(fallbackMessage(target, 'musl builds are not published yet') + '\n');
    process.exit(1);
  }

  const target = humanTarget({ platform, arch });
  let binaryPath;
  try {
    // Resolve relative to the metapackage root so the optional dependency
    // installed alongside it is found (npm hoists siblings into node_modules).
    const requireFromMetapackage = createRequire(path.join(__dirname, '..'));
    binaryPath = requireFromMetapackage.resolve(`${pkg}/bin/${binName}`);
  } catch (_) {
    process.stderr.write(
      fallbackMessage(
        target,
        `"${pkg}" is not installed — the install may have skipped optional dependencies (--no-optional)`,
      ) + '\n',
    );
    process.exit(1);
  }
  return execBinary(binaryPath, argv);
}

module.exports = {
  PACKAGE_SCOPE,
  CARGO_INSTALL_HINT,
  detectLinuxLibc,
  platformPackageName,
  humanTarget,
  fallbackMessage,
  execBinary,
  run,
};
