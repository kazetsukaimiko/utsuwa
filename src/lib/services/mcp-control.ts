import { isTauri } from '$lib/services/platform';
import { chatStore } from '$lib/stores/chat.svelte';
import { ttsStore } from '$lib/stores/tts.svelte';
import { vrmStore } from '$lib/stores/vrm.svelte';
import { personaStore } from '$lib/stores/persona.svelte';
import { modulesStore } from '$lib/stores/modules.svelte';
import { mcpSessionsStore } from '$lib/stores/mcp-sessions.svelte';
import { canSpeak } from '$lib/stores/tts-store-logic';
import { getCurrentTtsOptions, isSpeechEnabled } from '$lib/services/tts/session-options';
import { resolveMcpUserInstructions } from './mcp-mode';
import {
	parseMcpCommand,
	speakTextFromArgs,
	languageFromArgs,
	isCommandForWindow,
	type McpCommand
} from './mcp-control-logic';

export interface McpHooks {
	setLatestResponse: (text: string) => void;
}

export interface McpReply {
	ok: boolean;
	payload: Record<string, unknown>;
}

export function executeMcpCommand(cmd: McpCommand, hooks: McpHooks): McpReply {
	switch (cmd.tool) {
		case 'get_status':
			return statusReply();
		case 'stop_speech':
			ttsStore.stop();
			vrmStore.stopTalking();
			return { ok: true, payload: { stopped: true, speaking: ttsStore.isSpeaking } };
		case 'speak':
			return speak(cmd, hooks);
		default:
			return { ok: false, payload: { error: `unknown tool: ${cmd.tool}` } };
	}
}

function speak(cmd: McpCommand, hooks: McpHooks): McpReply {
	const text = speakTextFromArgs(cmd.arguments);
	if (typeof text !== 'string') return { ok: false, payload: text };

	const language = languageFromArgs(cmd.arguments);
	const spoken = text;

	chatStore.addMessage('assistant', spoken);
	hooks.setLatestResponse(spoken);
	mcpSessionsStore.completeWait(cmd.sessionId);

	const speaker = cmd.sessionId;
	if (!isSpeechEnabled()) {
		vrmStore.startTalking(spoken, speaker);
		return {
			ok: true,
			payload: { queued: false, spoken: false, warning: 'Speech module is disabled' }
		};
	}

	const options = getCurrentTtsOptions();
	if (!options) {
		vrmStore.startTalking(spoken, speaker);
		return {
			ok: true,
			payload: { queued: false, spoken: false, warning: 'No TTS provider configured' }
		};
	}

	if (language) options.language = language;
	if (cmd.speakerVoice) options.voiceId = cmd.speakerVoice;

	if (!canSpeak(options)) {
		vrmStore.startTalking(spoken, speaker);
		return {
			ok: true,
			payload: {
				queued: false,
				spoken: false,
				warning: 'TTS is not ready (missing API key for a cloud provider)'
			}
		};
	}

	vrmStore.startTalking(spoken, speaker);
	void ttsStore.speak(spoken, options);
	return { ok: true, payload: { queued: true, spoken: true, provider: options.provider } };
}

function statusReply(): McpReply {
	const options = getCurrentTtsOptions();
	return {
		ok: true,
		payload: {
			ready: true,
			speaking: ttsStore.isSpeaking,
			speechEnabled: isSpeechEnabled(),
			ttsProvider: options?.provider ?? null,
			ttsReady: options ? canSpeak(options) : false,
			characterName: personaStore.name,
			availableModels: vrmStore.models.map((m) => ({ id: m.id, name: m.name })),
			lastTtsError: ttsStore.lastError
		}
	};
}

/**
 * Listen for mcp:command events from the Tauri backend. No-op outside desktop.
 * Returns an unsubscribe function.
 */
export async function startMcpBridge(
	hooks: McpHooks,
	opts?: { spawnSessionAvatars?: boolean }
): Promise<() => void> {
	if (!isTauri()) return () => {};

	const { listen } = await import('@tauri-apps/api/event');
	const { invoke } = await import('@tauri-apps/api/core');
	const { getCurrentWindow } = await import('@tauri-apps/api/window');
	const windowLabel = getCurrentWindow().label;

	const unlistenCommand = await listen<unknown>(
		'mcp:command',
		async (event) => {
			const parsed = parseMcpCommand(event.payload);
			if ('error' in parsed) {
				const id =
					event.payload && typeof event.payload === 'object' && 'id' in event.payload
						? String((event.payload as { id: unknown }).id)
						: '';
				if (id) {
					await invoke('mcp_reply', { id, ok: false, payload: { error: parsed.error } });
				}
				return;
			}
			// listen() defaults to every window; ignore events meant for the other one
			// or both main and overlay will hit OmniVoice a beat apart.
			if (!isCommandForWindow(parsed, windowLabel)) return;
			try {
				const reply = executeMcpCommand(parsed, hooks);
				await invoke('mcp_reply', { id: parsed.id, ok: reply.ok, payload: reply.payload });
				if (parsed.tool === 'speak' && reply.ok) {
					const { emit } = await import('@tauri-apps/api/event');
					await emit('mcp:turn-complete', { sessionId: parsed.sessionId ?? null });
				}
			} catch (err) {
				await invoke('mcp_reply', {
					id: parsed.id,
					ok: false,
					payload: { error: err instanceof Error ? err.message : String(err) }
				});
			}
		},
		{ target: windowLabel }
	);

	const unlistenSessions = await mcpSessionsStore.start({
		spawnAvatars: opts?.spawnSessionAvatars === true
	});
	await syncMcpInstructions();

	return () => {
		unlistenCommand();
		unlistenSessions();
	};
}

/** Push the saved (or default) client-instructions prompt into the Rust MCP server. */
export async function syncMcpInstructions(text?: string): Promise<void> {
	if (!isTauri()) return;
	const raw =
		text ?? (modulesStore.getModuleSettings('consciousness').mcpInstructions as string | undefined);
	const { invoke } = await import('@tauri-apps/api/core');
	await invoke('mcp_set_instructions', { text: resolveMcpUserInstructions(raw) });
}
