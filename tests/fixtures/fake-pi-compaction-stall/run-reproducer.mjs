#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const fixtureDir = dirname(fileURLToPath(import.meta.url));
const extension = join(fixtureDir, "fixture-extension.ts");
const captureOnly = process.argv.includes("--capture-only");
const positional = process.argv.slice(2).filter((arg) => arg !== "--capture-only");
const outputDir = resolve(positional[0] ?? mkdtempSync(join(tmpdir(), "fake-pi-compaction-stall-")));
const pi = process.env.PI_BIN ?? "pi";
const recoveryMarker = "FIXTURE_RECOVERY_TURN_EXECUTED";

mkdirSync(outputDir, { recursive: true });
const home = join(outputDir, "home");
mkdirSync(join(home, ".pi", "agent"), { recursive: true });
writeFileSync(
	join(home, ".pi", "agent", "settings.json"),
	JSON.stringify(
		{
			compaction: { enabled: true, reserveTokens: 500, keepRecentTokens: 1 },
			retry: { enabled: false },
			quietStartup: true,
		},
		null,
		2,
	),
);

// Deliberately allowlist process state instead of forwarding the parent environment.
// In particular, no provider/API credential variables and no ambient Pi auth/session
// variables can reach the credential-free child process.
const baseEnv = {
	PATH: process.env.PATH ?? "/usr/bin:/bin",
	HOME: home,
	PI_OFFLINE: "1",
	NO_COLOR: "1",
	LANG: process.env.LANG ?? "C.UTF-8",
	...(process.env.TMPDIR ? { TMPDIR: process.env.TMPDIR } : {}),
};

const commonArgs = (sessionDir) => [
	"--offline",
	"--approve",
	"--no-extensions",
	"--no-skills",
	"--no-prompt-templates",
	"--no-context-files",
	"--no-builtin-tools",
	"--extension",
	extension,
	"--provider",
	"fake-pi-compaction-stall",
	"--model",
	"fake-long-agentic-turn",
	"--session-dir",
	sessionDir,
];

function parseJsonl(text, label) {
	return text
		.split("\n")
		.filter(Boolean)
		.map((line, index) => {
			try {
				return JSON.parse(line);
			} catch (error) {
				throw new Error(`${label}: invalid JSONL at line ${index + 1}: ${line}\n${error}`);
			}
		});
}

function findSessionFile(root) {
	const pending = [root];
	while (pending.length > 0) {
		const dir = pending.pop();
		for (const entry of readdirSync(dir, { withFileTypes: true })) {
			const path = join(dir, entry.name);
			if (entry.isDirectory()) pending.push(path);
			else if (entry.name.endsWith(".jsonl")) return path;
		}
	}
	throw new Error(`No session JSONL found under ${root}`);
}

function runJsonScenario(scenario, prompt) {
	const scenarioDir = join(outputDir, scenario);
	const sessionDir = join(scenarioDir, "sessions");
	mkdirSync(sessionDir, { recursive: true });
	const result = spawnSync(pi, ["--mode", "json", ...commonArgs(sessionDir), prompt], {
		cwd: fixtureDir,
		env: { ...baseEnv, FAKE_PI_SCENARIO: scenario },
		encoding: "utf8",
		timeout: 20_000,
	});
	writeFileSync(join(scenarioDir, "events.jsonl"), result.stdout ?? "");
	writeFileSync(join(scenarioDir, "stderr.txt"), result.stderr ?? "");
	assert.equal(result.error, undefined, `${scenario}: Pi process error: ${result.error}`);
	assert.equal(result.status, 0, `${scenario}: Pi exited ${result.status}: ${result.stderr}`);
	return {
		events: parseJsonl(result.stdout, scenario),
		sessionFile: findSessionFile(sessionDir),
	};
}

function runManualScenario() {
	return new Promise((resolvePromise, rejectPromise) => {
		const scenario = "manual";
		const scenarioDir = join(outputDir, scenario);
		const sessionDir = join(scenarioDir, "sessions");
		mkdirSync(sessionDir, { recursive: true });
		const child = spawn(pi, ["--mode", "rpc", ...commonArgs(sessionDir)], {
			cwd: fixtureDir,
			env: { ...baseEnv, FAKE_PI_SCENARIO: scenario },
			stdio: ["pipe", "pipe", "pipe"],
		});
		const records = [];
		const pendingResponses = new Map();
		const pendingEvents = [];
		let stdoutBuffer = "";
		let stderr = "";
		let request = 0;
		let settled = false;

		const reject = (error) => {
			if (settled) return;
			settled = true;
			child.kill("SIGKILL");
			rejectPromise(error);
		};
		const timer = setTimeout(() => reject(new Error(`manual RPC timeout; stderr=${stderr}`)), 20_000);

		function acceptRecord(record) {
			records.push(record);
			if (record.type === "response" && record.id && pendingResponses.has(record.id)) {
				const complete = pendingResponses.get(record.id);
				pendingResponses.delete(record.id);
				complete(record);
			}
			for (let i = pendingEvents.length - 1; i >= 0; i -= 1) {
				if (pendingEvents[i].predicate(record)) {
					const [{ complete }] = pendingEvents.splice(i, 1);
					complete(record);
				}
			}
		}

		child.stdout.on("data", (chunk) => {
			stdoutBuffer += chunk.toString();
			while (stdoutBuffer.includes("\n")) {
				const newline = stdoutBuffer.indexOf("\n");
				const line = stdoutBuffer.slice(0, newline).replace(/\r$/, "");
				stdoutBuffer = stdoutBuffer.slice(newline + 1);
				if (!line) continue;
				try {
					acceptRecord(JSON.parse(line));
				} catch (error) {
					reject(new Error(`manual: invalid RPC JSONL: ${line}\n${error}`));
				}
			}
		});
		child.stderr.on("data", (chunk) => {
			stderr += chunk.toString();
		});
		child.on("error", reject);
		child.on("exit", (code, signal) => {
			if (!settled) reject(new Error(`manual RPC exited early code=${code} signal=${signal}; stderr=${stderr}`));
		});

		function send(type, fields = {}) {
			const id = `fixture-${++request}`;
			const response = new Promise((complete) => pendingResponses.set(id, complete));
			child.stdin.write(`${JSON.stringify({ id, type, ...fields })}\n`);
			return response;
		}
		function waitForEvent(predicate) {
			return new Promise((complete) => pendingEvents.push({ predicate, complete }));
		}

		(async () => {
			const agentSettled = waitForEvent((record) => record.type === "agent_settled");
			const promptResponse = await send("prompt", { message: "Seed a session for explicit manual compaction." });
			assert.equal(promptResponse.success, true, `manual prompt failed: ${JSON.stringify(promptResponse)}`);
			await agentSettled;
			const compactResponse = await send("compact", { customInstructions: "manual fixture control" });
			assert.equal(compactResponse.success, true, `manual compact failed: ${JSON.stringify(compactResponse)}`);
			const entriesResponse = await send("get_entries");
			assert.equal(entriesResponse.success, true, `manual get_entries failed: ${JSON.stringify(entriesResponse)}`);

			writeFileSync(join(scenarioDir, "events.jsonl"), `${records.map((record) => JSON.stringify(record)).join("\n")}\n`);
			writeFileSync(join(scenarioDir, "stderr.txt"), stderr);
			clearTimeout(timer);
			settled = true;
			child.kill("SIGTERM");
			resolvePromise({ events: records, sessionFile: findSessionFile(sessionDir) });
		})().catch(reject);
	});
}

function eventText(event) {
	if (event?.message?.role !== "assistant") return "";
	return event.message.content
		.filter((block) => block.type === "text")
		.map((block) => block.text)
		.join("\n");
}

function sessionEntries(path) {
	return parseJsonl(readFileSync(path, "utf8"), path);
}

function successfulCompaction(events, reason) {
	const startIndex = events.findIndex((event) => event.type === "compaction_start" && event.reason === reason);
	assert.notEqual(startIndex, -1, `${reason}: missing compaction_start`);
	const endIndex = events.findIndex(
		(event, index) => index > startIndex && event.type === "compaction_end" && event.reason === reason,
	);
	assert.notEqual(endIndex, -1, `${reason}: missing compaction_end`);
	const end = events[endIndex];
	assert.equal(end.aborted, false, `${reason}: compaction was aborted`);
	assert.ok(end.result, `${reason}: compaction did not succeed: ${end.errorMessage ?? "no result"}`);
	return { startIndex, endIndex, end };
}

function compactTrace(label, events) {
	const lines = [`# ${label}`];
	for (const [index, event] of events.entries()) {
		if (["session", "response", "message_update", "message_start", "entry_appended"].includes(event.type)) continue;
		let suffix = "";
		if (event.type === "compaction_start") suffix = ` reason=${event.reason}`;
		if (event.type === "compaction_end") {
			suffix = ` reason=${event.reason} aborted=${event.aborted} willRetry=${event.willRetry} result=${Boolean(event.result)}`;
			if (event.errorMessage) suffix += ` error=${JSON.stringify(event.errorMessage)}`;
		}
		if (event.type === "agent_end") suffix = ` willRetry=${event.willRetry}`;
		if (event.type === "queue_update") {
			suffix = ` steering=${JSON.stringify(event.steering)} followUp=${JSON.stringify(event.followUp)}`;
		}
		if (event.type === "message_end") {
			suffix = ` role=${event.message.role}`;
			const text = eventText(event);
			if (text) suffix += ` text=${JSON.stringify(text)}`;
			if (event.message.stopReason) suffix += ` stopReason=${event.message.stopReason}`;
		}
		lines.push(`${String(index).padStart(3, "0")} ${event.type}${suffix}`);
	}
	return lines.join("\n");
}

const target = runJsonScenario(
	"threshold",
	"Perform the long fixture task: record intermediate progress, then execute one concrete finish-work provider turn and report completion.",
);
const overflow = runJsonScenario("overflow", "Exercise overflow compaction retry.");
const failed = runJsonScenario("failed", "Exercise failed threshold compaction.");
const agentEndFollowUp = runJsonScenario("agent-end-follow-up", "Exercise the agent_end queued follow-up control.");
const manual = await runManualScenario();

const overflowCompaction = successfulCompaction(overflow.events, "overflow");
assert.equal(overflowCompaction.end.willRetry, true, "overflow: successful recovery compaction must set willRetry=true");
assert.ok(
	overflow.events.slice(overflowCompaction.endIndex + 1).some((event) => eventText(event).includes("OVERFLOW_RETRY_CONTINUED")),
	"overflow: automatic retry did not produce the second provider response",
);

const manualCompaction = successfulCompaction(manual.events, "manual");
assert.equal(manualCompaction.end.willRetry, false, "manual: explicit compaction must not masquerade as overflow retry");
assert.ok(
	manual.events.some((event) => event.type === "response" && event.command === "compact" && event.success === true),
	"manual: missing successful explicit RPC compact response",
);

const failedEnd = failed.events.find((event) => event.type === "compaction_end" && event.reason === "threshold");
assert.ok(failedEnd, "failed: missing threshold compaction_end");
assert.equal(Boolean(failedEnd.result), false, "failed: compaction unexpectedly succeeded");
assert.match(
	failedEnd.errorMessage ?? "",
	/Auto-compaction failed: (?:Turn prefix )?[Ss]ummarization failed: fixture summarization failure/,
);
assert.equal(
	sessionEntries(failed.sessionFile).some((entry) => entry.type === "compaction"),
	false,
	"failed: an unsuccessful compaction must not append a compaction session entry",
);

const firstFollowUpAgentEnd = agentEndFollowUp.events.findIndex((event) => event.type === "agent_end");
assert.notEqual(firstFollowUpAgentEnd, -1, "agent_end follow-up: missing first agent_end");
assert.ok(
	agentEndFollowUp.events.some(
		(event) => event.type === "queue_update" && event.followUp?.includes("AGENT_END_QUEUED_FOLLOW_UP"),
	),
	"agent_end follow-up: extension did not queue its explicit follow-up",
);
assert.ok(
	agentEndFollowUp.events
		.slice(firstFollowUpAgentEnd + 1)
		.some((event) => eventText(event).includes("AGENT_END_FOLLOW_UP_CONTINUED")),
	"agent_end follow-up: explicit queued continuation did not run",
);

const targetCompaction = successfulCompaction(target.events, "threshold");
assert.equal(targetCompaction.end.willRetry, false, "threshold: expected willRetry=false");
assert.match(targetCompaction.end.result.summary, /UNFINISHED_WORK_STATE: true/);
assert.match(targetCompaction.end.result.summary, /NEXT_ACTION_EXECUTED: false/);
const targetCompactionEntry = sessionEntries(target.sessionFile).find((entry) => entry.type === "compaction");
assert.ok(targetCompactionEntry, "threshold: successful compaction was not persisted");
assert.equal(targetCompactionEntry.details?.unfinished, true, "threshold: session entry lost explicit unfinished state");
assert.equal(targetCompactionEntry.details?.nextActionExecuted, false, "threshold: session entry did not record unexecuted next action");
assert.match(targetCompactionEntry.summary, /NEXT_REQUIRED_ACTION: invoke the fake provider for one concrete finish-work turn/);
assert.equal(
	target.events.some(
		(event) => event.type === "queue_update" && ((event.steering?.length ?? 0) > 0 || (event.followUp?.length ?? 0) > 0),
	),
	false,
	"threshold: target unexpectedly had queued steering/follow-up",
);
const targetAfterCompaction = target.events.slice(targetCompaction.endIndex + 1);
const targetSettled = targetAfterCompaction.findIndex((event) => event.type === "agent_settled");
assert.notEqual(targetSettled, -1, "threshold: Pi did not emit agent_settled after compaction");

const trace = [
	compactTrace("threshold target", target.events),
	compactTrace("overflow control", overflow.events),
	compactTrace("manual RPC control", manual.events),
	compactTrace("failed compaction control", failed.events),
	compactTrace("agent_end queued-follow-up control", agentEndFollowUp.events),
].join("\n\n");
writeFileSync(join(outputDir, "trace.txt"), `${trace}\n`);
writeFileSync(
	join(outputDir, "session-paths.txt"),
	[
		`threshold=${target.sessionFile}`,
		`overflow=${overflow.sessionFile}`,
		`manual=${manual.sessionFile}`,
		`failed=${failed.sessionFile}`,
		`agent-end-follow-up=${agentEndFollowUp.sessionFile}`,
	].join("\n") + "\n",
);

console.log(`Pi fixture output: ${outputDir}`);
console.log(trace);
console.log("CONTROL ASSERTIONS: PASS (overflow, manual, failed compaction, agent_end queued follow-up)");

const targetRecoveryInterval = targetAfterCompaction.slice(0, targetSettled);
function orderedRecoveryTurn(events) {
	const agentStart = events.findIndex((event) => event.type === "agent_start");
	const turnStart = events.findIndex((event, index) => index > agentStart && event.type === "turn_start");
	const assistantMarker = events.findIndex(
		(event, index) =>
			index > turnStart && event.type === "message_end" && eventText(event).includes(recoveryMarker),
	);
	const turnEnd = events.findIndex((event, index) => index > assistantMarker && event.type === "turn_end");
	const agentEnd = events.findIndex((event, index) => index > turnEnd && event.type === "agent_end");
	return agentStart >= 0 && turnStart > agentStart && assistantMarker > turnStart && turnEnd > assistantMarker && agentEnd > turnEnd;
}
const hasOrderedRecoveryTurn = orderedRecoveryTurn(targetRecoveryInterval);
if (captureOnly) {
	assert.equal(hasOrderedRecoveryTurn, false, "capture-only: installed Pi no longer exhibits the threshold idle stall");
	assert.equal(
		targetRecoveryInterval.some((event) => event.type === "turn_start"),
		false,
		"capture-only: installed Pi unexpectedly started a post-compaction turn",
	);
	console.log("OBSERVED BUG: successful threshold compaction was followed by agent_settled with no concrete recovery turn");
	process.exit(0);
}

assert.ok(
	hasOrderedRecoveryTurn,
	`RED: threshold compaction with explicit unfinished work must schedule one concrete post-compaction recovery turn (expected assistant marker ${recoveryMarker} after successful compaction_end(willRetry=false))`,
);
