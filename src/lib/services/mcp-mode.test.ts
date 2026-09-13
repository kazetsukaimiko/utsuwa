import test from 'node:test';
import assert from 'node:assert/strict';
import {
	hostnameToSessionName,
	clampTopic,
	applySpeakTemplate,
	resolveSpokenText,
	isMcpProvider,
	DEFAULT_SPEAK_TEMPLATE,
	DEFAULT_MCP_INSTRUCTIONS,
	DEFAULT_MCP_USER_INSTRUCTIONS,
	HARDCODED_MCP_INSTRUCTIONS,
	composeMcpInstructions,
	resolveMcpInstructions,
	resolveMcpUserInstructions,
	isLegacyMcpInstructions
} from './mcp-mode.ts';

test('hostnameToSessionName strips domain and title-cases', () => {
	assert.equal(hostnameToSessionName('shizuku.local'), 'Shizuku');
	assert.equal(hostnameToSessionName('shizuku'), 'Shizuku');
	assert.equal(hostnameToSessionName('  Agnea.local  '), 'Agnea');
	assert.equal(hostnameToSessionName(''), 'Host');
});

test('clampTopic keeps at most seven words', () => {
	assert.equal(clampTopic('utsuwa MCP chat'), 'utsuwa MCP chat');
	assert.equal(
		clampTopic('one two three four five six seven eight nine'),
		'one two three four five six seven'
	);
	assert.equal(clampTopic('  spaced   words  '), 'spaced words');
});

test('applySpeakTemplate fills name topic and message', () => {
	assert.equal(
		applySpeakTemplate(DEFAULT_SPEAK_TEMPLATE, {
			name: 'Shizuku',
			topic: 'utsuwa build',
			message: 'they finished the new utsuwa build'
		}),
		'Shizuku says: they finished the new utsuwa build'
	);
	assert.equal(
		applySpeakTemplate('${name} tells me ${message}', {
			name: 'Agnea',
			topic: '',
			message: 'it finished updating hakobune'
		}),
		'Agnea tells me it finished updating hakobune'
	);
	assert.equal(
		applySpeakTemplate('  ', { name: 'Daphne', topic: '', message: 'needs a file permission' }),
		'Daphne says: needs a file permission'
	);
});

test('hardcoded MCP instructions cover polling and dumps, not user examples', () => {
	assert.match(HARDCODED_MCP_INSTRUCTIONS, /notification channel/);
	assert.match(HARDCODED_MCP_INSTRUCTIONS, /take_user_message/);
	assert.match(HARDCODED_MCP_INSTRUCTIONS, /AGENT_SESSION_ID/);
	assert.match(HARDCODED_MCP_INSTRUCTIONS, /First action after connect/);
	assert.match(HARDCODED_MCP_INSTRUCTIONS, /user utterance/);
	assert.match(HARDCODED_MCP_INSTRUCTIONS, /userAgent/);
	assert.match(HARDCODED_MCP_INSTRUCTIONS, /Never speak code/);
	assert.match(HARDCODED_MCP_INSTRUCTIONS, /does not replace speak/);
	assert.match(HARDCODED_MCP_INSTRUCTIONS, /coding turn/);
	assert.doesNotMatch(HARDCODED_MCP_INSTRUCTIONS, /Building the session picker/);
	assert.doesNotMatch(HARDCODED_MCP_INSTRUCTIONS, /Java is too old/);
	assert.doesNotMatch(HARDCODED_MCP_INSTRUCTIONS, /Ironing out the bugs/);
});

test('user MCP preferences use general examples', () => {
	assert.match(DEFAULT_MCP_USER_INSTRUCTIONS, /Starting the search UI/);
	assert.match(DEFAULT_MCP_USER_INSTRUCTIONS, /Working through a few errors/);
	assert.doesNotMatch(DEFAULT_MCP_USER_INSTRUCTIONS, /Building the session picker/);
	assert.doesNotMatch(DEFAULT_MCP_USER_INSTRUCTIONS, /Java is too old/);
	assert.doesNotMatch(DEFAULT_MCP_USER_INSTRUCTIONS, /Poll take_user_message/);
});

test('composeMcpInstructions always includes hardcoded usage plus preferences', () => {
	const composed = composeMcpInstructions('Be extremely terse.');
	assert.match(composed, /take_user_message/);
	assert.match(composed, /Preferences \(from the user at this machine\)/);
	assert.match(composed, /Be extremely terse\./);
	assert.doesNotMatch(composed, /Starting the search UI/);
});

test('resolveMcpInstructions falls back to the composed default', () => {
	assert.equal(resolveMcpInstructions('Be terse.'), composeMcpInstructions('Be terse.'));
	assert.equal(resolveMcpInstructions('  '), DEFAULT_MCP_INSTRUCTIONS);
	assert.equal(resolveMcpInstructions(undefined), DEFAULT_MCP_INSTRUCTIONS);
});

test('legacy full-blob instructions migrate to the user default', () => {
	assert.equal(isLegacyMcpInstructions('Building the session picker, I think I have an idea'), true);
	assert.equal(
		resolveMcpUserInstructions('Java is too old — can you install 21?'),
		DEFAULT_MCP_USER_INSTRUCTIONS
	);
	assert.equal(resolveMcpUserInstructions('Be terse.'), 'Be terse.');
});

test('isMcpProvider', () => {
	assert.equal(isMcpProvider('mcp'), true);
	assert.equal(isMcpProvider('openai'), false);
	assert.equal(isMcpProvider(undefined), false);
});

test('resolveSpokenText wraps unless plain', () => {
	assert.equal(
		resolveSpokenText(DEFAULT_SPEAK_TEMPLATE, 'they finished the new utsuwa build', {
			name: 'Shizuku',
			topic: 'utsuwa MCP chat'
		}),
		'Shizuku says: they finished the new utsuwa build'
	);
	assert.equal(
		resolveSpokenText('${name} tells me ${message}', 'it finished updating hakobune', {
			name: 'Agnea'
		}),
		'Agnea tells me it finished updating hakobune'
	);
	assert.equal(
		resolveSpokenText(DEFAULT_SPEAK_TEMPLATE, 'hello from grok', { name: 'Shizuku' }, true),
		'hello from grok'
	);
	assert.equal(
		resolveSpokenText(undefined, 'needs a file permission', { name: 'Daphne' }),
		'Daphne says: needs a file permission'
	);
});
