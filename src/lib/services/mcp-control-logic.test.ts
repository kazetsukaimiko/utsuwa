import test from 'node:test';
import assert from 'node:assert/strict';
import {
	parseMcpCommand,
	speakTextFromArgs,
	languageFromArgs,
	plainFromArgs,
	isCommandForWindow
} from './mcp-control-logic.ts';

test('parseMcpCommand accepts speak with arguments', () => {
	const parsed = parseMcpCommand({
		id: 'mcp-1',
		tool: 'speak',
		arguments: { text: 'hello', language: 'en' }
	});
	assert.ok(!('error' in parsed));
	if ('error' in parsed) return;
	assert.equal(parsed.tool, 'speak');
	assert.equal(parsed.arguments.text, 'hello');
});

test('parseMcpCommand rejects unknown tools and missing ids', () => {
	assert.equal(
		(parseMcpCommand({ id: 'x', tool: 'explode' }) as { error: string }).error,
		'unknown tool: explode'
	);
	assert.equal((parseMcpCommand({ tool: 'speak' }) as { error: string }).error, 'missing id');
	assert.equal((parseMcpCommand(null) as { error: string }).error, 'invalid command');
});

test('parseMcpCommand defaults missing arguments to an empty object', () => {
	const parsed = parseMcpCommand({ id: 'mcp-2', tool: 'get_status' });
	assert.ok(!('error' in parsed));
	if ('error' in parsed) return;
	assert.deepEqual(parsed.arguments, {});
});

test('speakTextFromArgs requires non-empty text', () => {
	assert.equal(speakTextFromArgs({ text: '  hi  ' }), 'hi');
	assert.deepEqual(speakTextFromArgs({ text: '   ' }), { error: 'text is required' });
	assert.deepEqual(speakTextFromArgs({}), { error: 'text is required' });
});

test('isCommandForWindow ignores events aimed at another window', () => {
	const cmd = parseMcpCommand({
		id: 'mcp-3',
		tool: 'speak',
		target: 'main',
		arguments: { text: 'hi' }
	});
	assert.ok(!('error' in cmd));
	if ('error' in cmd) return;
	assert.equal(isCommandForWindow(cmd, 'main'), true);
	assert.equal(isCommandForWindow(cmd, 'overlay'), false);
	assert.equal(isCommandForWindow({ ...cmd, target: undefined }, 'overlay'), true);
});

test('languageFromArgs ignores short or non-string values', () => {
	assert.equal(languageFromArgs({ language: 'en' }), 'en');
	assert.equal(languageFromArgs({ language: 'x' }), undefined);
	assert.equal(languageFromArgs({ language: 1 }), undefined);
});

test('plainFromArgs is true only for boolean true', () => {
	assert.equal(plainFromArgs({ plain: true }), true);
	assert.equal(plainFromArgs({ plain: false }), false);
	assert.equal(plainFromArgs({ plain: 'true' }), false);
	assert.equal(plainFromArgs({}), false);
});

test('parseMcpCommand accepts set_character', () => {
	const parsed = parseMcpCommand({
		id: 'mcp-3',
		tool: 'set_character',
		arguments: { modelId: 'default-vita' }
	});
	assert.ok(!('error' in parsed));
});

test('parseMcpCommand keeps session speaker fields', () => {
	const parsed = parseMcpCommand({
		id: 'mcp-4',
		tool: 'speak',
		sessionId: 'sess-1',
		speakerName: 'Shizuku',
		speakerTopic: 'utsuwa MCP chat',
		arguments: { text: 'hi', plain: true }
	});
	assert.ok(!('error' in parsed));
	if ('error' in parsed) return;
	assert.equal(parsed.sessionId, 'sess-1');
	assert.equal(parsed.speakerName, 'Shizuku');
	assert.equal(parsed.speakerTopic, 'utsuwa MCP chat');
	assert.equal(parsed.speakerVoice, undefined);
	assert.equal(plainFromArgs(parsed.arguments), true);
});
