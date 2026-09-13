import { isTauri } from '$lib/services/platform';
import { chatStore } from '$lib/stores/chat.svelte';

const hasWindow = typeof window !== 'undefined';

interface McpWaitHooks {
	setTyping: (typing: boolean) => void;
}

const SELECTED_KEY = 'utsuwa-mcp-selected-session';

export interface McpClientSession {
	id: string;
	name: string;
	topic: string;
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

function createMcpSessionsStore() {
	let sessions = $state<McpClientSession[]>([]);
	let selectedId = $state<string | null>(null);
	let hostName = $state('Host');
	let pendingWait: { sessionId: string; hooks: McpWaitHooks } | null = null;

	function applyList(list: McpClientSession[]) {
		sessions = list;
		const next = pickDefault(list, selectedId);
		if (next !== selectedId) selectedId = next;
		if (hasWindow && selectedId) localStorage.setItem(SELECTED_KEY, selectedId);
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

	async function start(): Promise<() => void> {
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
		select,
		beginWait,
		completeWait,
		cancelWait,
		start
	};
}

export const mcpSessionsStore = createMcpSessionsStore();
