import { isTauri } from '$lib/services/platform';
import { chatStore } from '$lib/stores/chat.svelte';
import { vrmStore } from '$lib/stores/vrm.svelte';

const hasWindow = typeof window !== 'undefined';

interface McpWaitHooks {
	setTyping: (typing: boolean) => void;
}

const SELECTED_KEY = 'utsuwa-mcp-selected-session';

export interface McpClientSession {
	id: string;
	name: string;
	topic: string;
	modelId: string;
	voiceId: string;
	resumeId: string;
	userAgent: string;
	pending: number;
}

function normalizeList(raw: unknown): McpClientSession[] {
	if (!Array.isArray(raw)) return [];
	const out: McpClientSession[] = [];
	for (const item of raw) {
		if (!item || typeof item !== 'object') continue;
		const obj = item as Record<string, unknown>;
		if (typeof obj.id !== 'string' || !obj.id) continue;
		out.push({
			id: obj.id,
			name: typeof obj.name === 'string' && obj.name ? obj.name : 'Host',
			topic: typeof obj.topic === 'string' ? obj.topic : '',
			modelId: typeof obj.modelId === 'string' ? obj.modelId : '',
			voiceId: typeof obj.voiceId === 'string' ? obj.voiceId : '',
			resumeId: typeof obj.resumeId === 'string' ? obj.resumeId : '',
			userAgent: typeof obj.userAgent === 'string' ? obj.userAgent : '',
			pending: typeof obj.pending === 'number' ? obj.pending : 0
		});
	}
	return out;
}

function pickDefault(list: McpClientSession[], current: string | null): string | null {
	if (current && list.some((s) => s.id === current)) return current;
	const saved = hasWindow ? localStorage.getItem(SELECTED_KEY) : null;
	if (saved && list.some((s) => s.id === saved)) return saved;
	const waiting = list.find((s) => s.pending > 0);
	return waiting?.id ?? list[0]?.id ?? null;
}

function resolveGalleryModel(hint: string | undefined) {
	if (!hint) return vrmStore.getActiveModel();
	const lower = hint.toLowerCase();
	return (
		vrmStore.models.find((m) => m.id === hint || m.name.toLowerCase() === lower) ??
		vrmStore.getActiveModel()
	);
}

function syncSessionAvatars(list: McpClientSession[]) {
	const ids = new Set(list.map((s) => s.id));
	for (const session of list) {
		const model = resolveGalleryModel(session.modelId);
		const existing = vrmStore.instances.find((inst) => inst.id === session.id);
		if (!existing) {
			vrmStore.spawnInstance(session.id, { modelId: model?.id, url: model?.url });
		} else if (model && existing.modelId !== model.id) {
			vrmStore.setInstanceModel(session.id, model.id);
		}
	}
	for (const inst of vrmStore.instances) {
		if (!inst.isPrimary && !ids.has(inst.id)) vrmStore.despawnInstance(inst.id);
	}
}

function createMcpSessionsStore() {
	let sessions = $state<McpClientSession[]>([]);
	let selectedId = $state<string | null>(null);
	let hostName = $state('Host');
	let pendingWait: { sessionId: string; hooks: McpWaitHooks } | null = null;
	let spawnAvatars = false;

	function applyList(list: McpClientSession[]) {
		sessions = list;
		const next = pickDefault(list, selectedId);
		if (next !== selectedId) selectedId = next;
		if (hasWindow && selectedId) localStorage.setItem(SELECTED_KEY, selectedId);
		if (spawnAvatars) syncSessionAvatars(list);
	}

	function select(id: string) {
		if (!sessions.some((s) => s.id === id)) return;
		selectedId = id;
		if (hasWindow) localStorage.setItem(SELECTED_KEY, id);
	}

	function beginWait(sessionId: string, hooks: McpWaitHooks) {
		pendingWait = { sessionId, hooks };
	}

	function completeWait(sessionId: string | undefined): boolean {
		if (!pendingWait) return false;
		if (sessionId && pendingWait.sessionId !== sessionId) return false;
		pendingWait.hooks.setTyping(false);
		chatStore.setLoading(false);
		pendingWait = null;
		return true;
	}

	function cancelWait(reason?: string) {
		if (!pendingWait) return;
		pendingWait.hooks.setTyping(false);
		chatStore.setLoading(false);
		if (reason) chatStore.setError(reason);
		pendingWait = null;
	}

	async function start(opts?: { spawnAvatars?: boolean }): Promise<() => void> {
		spawnAvatars = opts?.spawnAvatars === true;
		if (!hasWindow || !isTauri()) return () => {};

		const { listen } = await import('@tauri-apps/api/event');
		const { invoke } = await import('@tauri-apps/api/core');

		try {
			hostName = await invoke<string>('mcp_host_name');
		} catch {
			// keep default
		}
		try {
			applyList(normalizeList(await invoke('mcp_list_sessions')));
		} catch {
			applyList([]);
		}

		const unlistenSessions = await listen<unknown>('mcp:sessions-changed', (event) => {
			applyList(normalizeList(event.payload));
		});
		const unlistenTurn = await listen<{ sessionId?: string }>('mcp:turn-complete', (event) => {
			completeWait(event.payload?.sessionId);
		});
		return () => {
			unlistenSessions();
			unlistenTurn();
		};
	}

	return {
		get sessions() {
			return sessions;
		},
		get selectedId() {
			return selectedId;
		},
		get selected() {
			return sessions.find((s) => s.id === selectedId) ?? null;
		},
		get hostName() {
			return hostName;
		},
		get waiting() {
			return pendingWait !== null;
		},
		get spawnAvatars() {
			return spawnAvatars;
		},
		select,
		beginWait,
		completeWait,
		cancelWait,
		start
	};
}

export const mcpSessionsStore = createMcpSessionsStore();
