#!/usr/bin/env node
// Controlled Pi RPC wire fixture for the production `wg pi-process-worker`
// entry point. It models only events emitted by Pi + @mjakl/pi-processes; it is
// not a model/provider substitute for the separately retained real-Pi proof.
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";

const scenario = process.env.WG_PI_PROCESS_TEST_SCENARIO;
const emit = (value) => process.stdout.write(`${JSON.stringify(value)}\n`);
const started = (command = "node neutral-command.mjs") => emit({
  type: "tool_execution_end",
  toolCallId: "start-1",
  toolName: "process",
  result: {
    details: {
      action: "start",
      success: true,
      message: "Started\nLogs: /bounded/owned/proc_1-stdout.log",
      process: { id: "proc_1", pid: process.pid + 1, command },
    },
  },
  isError: false,
});
const wake = (exitCode) => emit({
  type: "message_start",
  message: {
    role: "custom",
    customType: "pi-processes:update",
    details: {
      processId: "proc_1",
      status: "exited",
      exitCode,
      success: exitCode === 0,
    },
  },
});

if (scenario === "race-success" || scenario === "race-nonzero") {
  const exitCode = scenario === "race-success" ? 0 : 7;
  started();
  // The completion and its duplicate arrive before the yielded turn boundary.
  wake(exitCode);
  wake(exitCode);
  emit({ type: "turn_end", turnId: "yielded-turn" });
  emit({ type: "message_start", message: { role: "assistant", content: "ONE_CONTINUATION" } });
  emit({ type: "turn_end", turnId: "continuation-turn" });
  emit({ type: "agent_end" });
  setInterval(() => {}, 1000);
} else if (scenario === "disconnect") {
  started();
  setTimeout(() => process.exit(0), 10);
} else if (scenario === "owned-cancellation") {
  const fixture = process.env.WG_PI_PROCESS_LONG_FIXTURE;
  const pidFile = process.env.WG_PI_PROCESS_CHILD_PID_FILE;
  if (!fixture || !pidFile) throw new Error("missing cancellation fixture environment");
  const child = spawn(process.execPath, [fixture, "60000", "0", "SHOULD_NOT_FINISH"], {
    stdio: "ignore",
  });
  writeFileSync(pidFile, `${child.pid}\n`);
  started(`${process.execPath} ${fixture} 60000 0 SHOULD_NOT_FINISH`);
  let stopping = false;
  const stop = () => {
    if (stopping) return;
    stopping = true;
    child.kill("SIGTERM");
    const force = setTimeout(() => child.kill("SIGKILL"), 500);
    child.once("exit", () => {
      clearTimeout(force);
      process.exit(143);
    });
  };
  process.on("SIGTERM", stop);
  process.on("SIGINT", stop);
  setInterval(() => {}, 1000);
} else {
  throw new Error(`unknown WG_PI_PROCESS_TEST_SCENARIO=${scenario}`);
}
