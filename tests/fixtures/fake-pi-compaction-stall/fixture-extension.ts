import {
	createAssistantMessageEventStream,
	type Api,
	type AssistantMessage,
	type AssistantMessageEventStream,
	type Context,
	type Model,
	type SimpleStreamOptions,
} from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

const PROVIDER = "fake-pi-compaction-stall";
const MODEL = "fake-long-agentic-turn";
const RECOVERY_MARKER = "FIXTURE_RECOVERY_TURN_EXECUTED";

function usage(totalTokens: number) {
	return {
		input: totalTokens - 10,
		output: 10,
		cacheRead: 0,
		cacheWrite: 0,
		totalTokens,
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	};
}

function newMessage(model: Model<Api>, totalTokens: number): AssistantMessage {
	return {
		role: "assistant",
		content: [],
		api: model.api,
		provider: model.provider,
		model: model.id,
		usage: usage(totalTokens),
		stopReason: "pending",
		timestamp: Date.now(),
	};
}

function textResponse(model: Model<Api>, text: string, totalTokens: number): AssistantMessageEventStream {
	const stream = createAssistantMessageEventStream();
	const output = newMessage(model, totalTokens);
	const block = { type: "text" as const, text };
	output.content.push(block);
	output.stopReason = "stop";
	stream.push({ type: "start", partial: output });
	stream.push({ type: "text_start", contentIndex: 0, partial: output });
	stream.push({ type: "text_delta", contentIndex: 0, delta: text, partial: output });
	stream.push({ type: "text_end", contentIndex: 0, content: text, partial: output });
	stream.push({ type: "done", reason: "stop", message: output });
	stream.end();
	return stream;
}

function toolResponse(model: Model<Api>): AssistantMessageEventStream {
	const stream = createAssistantMessageEventStream();
	const output = newMessage(model, 200);
	const toolCall = {
		type: "toolCall" as const,
		id: "fixture-progress-call",
		name: "fixture_progress",
		arguments: { step: "prepare-recovery-input" },
	};
	output.content.push(toolCall);
	output.stopReason = "toolUse";
	stream.push({ type: "start", partial: output });
	stream.push({ type: "toolcall_start", contentIndex: 0, partial: output });
	stream.push({
		type: "toolcall_delta",
		contentIndex: 0,
		delta: JSON.stringify(toolCall.arguments),
		partial: output,
	});
	stream.push({ type: "toolcall_end", contentIndex: 0, toolCall, partial: output });
	stream.push({ type: "done", reason: "toolUse", message: output });
	stream.end();
	return stream;
}

function errorResponse(model: Model<Api>, errorMessage: string, totalTokens: number): AssistantMessageEventStream {
	const stream = createAssistantMessageEventStream();
	const output = newMessage(model, totalTokens);
	output.stopReason = "error";
	output.errorMessage = errorMessage;
	stream.push({ type: "start", partial: output });
	stream.push({ type: "error", reason: "error", error: output });
	stream.end();
	return stream;
}

export default function fixture(pi: ExtensionAPI) {
	const scenario = process.env.FAKE_PI_SCENARIO ?? "threshold";
	let providerCalls = 0;
	let queuedAgentEndFollowUp = false;

	pi.registerTool({
		name: "fixture_progress",
		label: "Fixture progress",
		description: "Record a deterministic intermediate step for the compaction-stall fixture",
		parameters: Type.Object({ step: Type.String() }),
		async execute(_toolCallId, params) {
			return {
				content: [{ type: "text", text: `RECORDED_INTERMEDIATE_STEP:${params.step}` }],
				details: { recorded: true, step: params.step },
			};
		},
	});

	pi.registerProvider(PROVIDER, {
		name: "Credential-free compaction-stall fixture",
		baseUrl: "http://127.0.0.1.invalid",
		apiKey: "credential-free-fixture",
		api: "fake-pi-compaction-stall-api" as Api,
		models: [
			{
				id: MODEL,
				name: "Fake long agentic turn",
				reasoning: false,
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 2000,
				maxTokens: 512,
			},
		],
		streamSimple(model: Model<Api>, _context: Context, _options?: SimpleStreamOptions) {
			providerCalls += 1;

			if (scenario === "threshold") {
				if (providerCalls === 1) return toolResponse(model);
				if (providerCalls === 2) {
					return textResponse(
						model,
						"Preparation is complete. The required finish-work action has NOT been executed.",
						1700,
					);
				}
				return textResponse(model, RECOVERY_MARKER, 200);
			}

			if (scenario === "overflow") {
				if (providerCalls === 1) return toolResponse(model);
				if (providerCalls === 2) {
					return errorResponse(model, "context_length_exceeded: deterministic fixture overflow", 2100);
				}
				return textResponse(model, `OVERFLOW_RETRY_CONTINUED ${RECOVERY_MARKER}`, 200);
			}

			if (scenario === "failed") {
				if (providerCalls === 1) {
					return textResponse(model, "High-usage response before compaction failure.", 1700);
				}
				return errorResponse(model, "fixture summarization failure", 20);
			}

			if (scenario === "agent-end-follow-up") {
				return textResponse(
					model,
					providerCalls === 1 ? "First low-usage run complete." : `AGENT_END_FOLLOW_UP_CONTINUED ${RECOVERY_MARKER}`,
					200,
				);
			}

			return textResponse(model, "Manual-control seed response.", 200);
		},
	});

	pi.on("session_before_compact", (event) => {
		if (scenario === "failed") return;
		return {
			compaction: {
				summary: [
					"## Explicit fixture state",
					"UNFINISHED_WORK_STATE: true",
					"NEXT_REQUIRED_ACTION: invoke the fake provider for one concrete finish-work turn",
					"NEXT_ACTION_EXECUTED: false",
					`COMPACTION_REASON: ${event.reason}`,
				].join("\n"),
				firstKeptEntryId: event.preparation.firstKeptEntryId,
				tokensBefore: event.preparation.tokensBefore,
				details: {
					unfinished: true,
					nextRequiredAction: "finish-work provider turn",
					nextActionExecuted: false,
					reason: event.reason,
				},
			},
		};
	});

	pi.on("agent_end", () => {
		if (scenario !== "agent-end-follow-up" || queuedAgentEndFollowUp) return;
		queuedAgentEndFollowUp = true;
		pi.sendUserMessage("AGENT_END_QUEUED_FOLLOW_UP", { deliverAs: "followUp" });
	});
}
