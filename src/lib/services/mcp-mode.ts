export const MCP_PROVIDER_ID = 'mcp';
export const MCP_MODEL_ID = 'mcp';
export const DEFAULT_SPEAK_TEMPLATE = '${name} says: ${message}';
export const MCP_TOPIC_MAX_WORDS = 7;

/**
 * Always prepended on initialize. Not user-editable — clients must keep polling
 * even if the preferences box is emptied. Keep in sync with src-tauri/src/mcp.rs.
 */
export const HARDCODED_MCP_INSTRUCTIONS = `Utsuwa is a brief notification channel to the person at this machine: speech bubble, voice, and lip-sync. It is not a transcript of your work.

Usage (do not ignore this section):
- Do the actual work in this session as usual, whether the user spoke in this TUI or sent a line through Utsuwa's chat bar.
- Chat-bar lines arrive via take_user_message as prompt. That is a real user message; answer it.
- You only receive what they routed to this session. Immediately after connect, call set_session — that is what makes your avatar appear; initialize alone does not. Pass name (hostname default), topic (1-7 words), sessionId (env AGENT_SESSION_ID), and userAgent (your client product name). Reconnects with the same sessionId resume the same avatar.
- Poll take_user_message regularly, including while idle between TUI turns. If you stop polling, their chat-bar lines sit unseen.
- Call speak with only the spoken payload in text (one or two sentences). Never speak code, diffs, logs, stack traces, or essays. Do not repeat the same status.
- Pass plain: true only when the line must be said exactly as written.
- A reply in this TUI does not replace speak(). Notify via speak at plan, blocker, and done even when the user asked here.
- Do not stay silent through a long stretch of tool use. If you have not spoken in a while, send one short status line. "This is a coding turn" is not a reason to skip speak.

Default cadence (overridden by Preferences below):
- When you have a plan: one short line that you are starting, and that you see a way forward.
- When you are stuck on something they must fix: one line plus what you need from them.
- When you finish: say you are done.
- While grinding through routine errors: stay vague. Do not narrate every failure.`;

/** Default contents of the settings textarea. Keep in sync with src-tauri/src/mcp.rs. */
export const DEFAULT_MCP_USER_INSTRUCTIONS = `Keep updates short and spoken-friendly. A few per task is enough — not every tool call.

Good:
- "Starting the search UI — I have a plan."
- "Need a newer runtime before this will build. Can you install it?"
- "Working through a few errors."
- "That's in place."

Avoid long explanations in speak(); put those in the TUI.`;

export function isLegacyMcpInstructions(text: string): boolean {
	return (
		text.includes('Building the session picker, I think I have an idea') ||
		text.includes('Java is too old') ||
		text.includes('Ironing out the bugs now')
	);
}

/** User-preferences box only (not the hardcoded usage block). */
export function resolveMcpUserInstructions(raw: string | undefined | null): string {
	const text = raw?.trim();
	if (!text || isLegacyMcpInstructions(text)) return DEFAULT_MCP_USER_INSTRUCTIONS;
	return text;
}

export function composeMcpInstructions(userRaw?: string | null): string {
	return `${HARDCODED_MCP_INSTRUCTIONS}\n\nPreferences (from the user at this machine):\n${resolveMcpUserInstructions(userRaw)}`;
}

export function resolveMcpInstructions(raw: string | undefined | null): string {
	return composeMcpInstructions(raw);
}

/** Composed default (hardcoded usage + default preferences). */
export const DEFAULT_MCP_INSTRUCTIONS = composeMcpInstructions(null);

export function isMcpProvider(id: string | undefined | null): boolean {
	return id === MCP_PROVIDER_ID;
}

/** shizuku.local / shizuku → Shizuku */
export function hostnameToSessionName(hostname: string): string {
	const stem = hostname.trim().split('.')[0] ?? '';
	if (!stem) return 'Host';
	return stem.charAt(0).toUpperCase() + stem.slice(1);
}

export function clampTopic(raw: string): string {
	const words = raw
		.trim()
		.split(/\s+/)
		.filter(Boolean)
		.slice(0, MCP_TOPIC_MAX_WORDS);
	return words.join(' ');
}

export function applySpeakTemplate(
	template: string,
	vars: { name: string; topic: string; message: string }
): string {
	const t = template.trim() || DEFAULT_SPEAK_TEMPLATE;
	return t
		.replaceAll('${name}', vars.name)
		.replaceAll('${topic}', vars.topic)
		.replaceAll('${message}', vars.message);
}

/** Wrap a speak payload unless the client asked for a verbatim line. */
export function resolveSpokenText(
	template: string | undefined,
	message: string,
	speaker: { name?: string; topic?: string },
	plain?: boolean
): string {
	if (plain) return message;
	return applySpeakTemplate(template ?? DEFAULT_SPEAK_TEMPLATE, {
		name: speaker.name?.trim() || 'Someone',
		topic: speaker.topic ?? '',
		message
	});
}
