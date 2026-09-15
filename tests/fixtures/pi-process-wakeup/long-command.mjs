const delayMs = Number(process.argv[2] ?? "1500");
const exitCode = Number(process.argv[3] ?? "0");
const marker = process.argv[4] ?? "WG_PI_PROCESS_DONE";

setTimeout(() => {
  process.stdout.write(`${marker}\n`);
  process.exit(exitCode);
}, delayMs);
