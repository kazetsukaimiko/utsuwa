export const MCP_TOOLS = [
	'speak',
	'stop_speech',
	'get_status',
	'debug_state',
	'debug_chat_bar',
	'set_session',
	'set_character',
	'set_voice',
	'take_user_message'
] as const;
export type McpTool = (typeof MCP_TOOLS)[number];

export interface McpCommand {
	id: string;
	tool: McpTool;
	/** Window label the backend targeted. Other windows must ignore the event. */
	target?: string;
	sessionId?: string;
	speakerName?: string;
	speakerTopic?: string;
	speakerVoice?: string;
	arguments: Record<string, unknown>;
}

const TOOL_SET: Set<string> = new Set(MCP_TOOLS);

export function parseMcpCommand(raw: unknown): McpCommand | { error: string } {
	if (!raw || typeof raw !== 'object') return { error: 'invalid command' };
	const obj = raw as Record<string, unknown>;
	if (typeof obj.id !== 'string' || !obj.id) return { error: 'missing id' };
	if (typeof obj.tool !== 'string' || !TOOL_SET.has(obj.tool)) {
		return { error: `unknown tool: ${String(obj.tool)}` };
	}
	const args =
		obj.arguments && typeof obj.arguments === 'object' && !Array.isArray(obj.arguments)
			? (obj.arguments as Record<string, unknown>)
			: {};
	const target = typeof obj.target === 'string' && obj.target ? obj.target : undefined;
	const sessionId =
		typeof obj.sessionId === 'string' && obj.sessionId ? obj.sessionId : undefined;
	const speakerName =
		typeof obj.speakerName === 'string' && obj.speakerName ? obj.speakerName : undefined;
	const speakerTopic = typeof obj.speakerTopic === 'string' ? obj.speakerTopic : undefined;
	const speakerVoice =
		typeof obj.speakerVoice === 'string' && obj.speakerVoice ? obj.speakerVoice : undefined;
	return {
		id: obj.id,
		tool: obj.tool as McpTool,
		target,
		sessionId,
		speakerName,
		speakerTopic,
		speakerVoice,
		arguments: args
	};
}

/** Main and overlay both subscribe; only the named window may run TTS. */
export function isCommandForWindow(cmd: McpCommand, windowLabel: string): boolean {
	if (!cmd.target) return true;
	return cmd.target === windowLabel;
}

export function speakTextFromArgs(args: Record<string, unknown>): string | { error: string } {
	const text = typeof args.text === 'string' ? args.text.trim() : '';
	if (!text) return { error: 'text is required' };
	return text;
}

export function languageFromArgs(args: Record<string, unknown>): string | undefined {
	if (typeof args.language !== 'string') return undefined;
	const lang = args.language.trim();
	return lang.length >= 2 ? lang : undefined;
}

export function plainFromArgs(args: Record<string, unknown>): boolean {
	return args.plain === true;
}
