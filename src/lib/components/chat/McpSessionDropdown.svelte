<script lang="ts">
	import { DropdownMenu } from 'bits-ui';
	import { Icon } from '$lib/components/ui';
	import { mcpSessionsStore } from '$lib/stores/mcp-sessions.svelte';

	interface Props {
		/** Short chip beside the floating chat pill. */
		compact?: boolean;
	}

	let { compact = false }: Props = $props();

	let open = $state(false);

	const selected = $derived(mcpSessionsStore.selected);
	const sessions = $derived(mcpSessionsStore.sessions);
</script>

<div class="mcp-session-picker" class:compact>
	<DropdownMenu.Root bind:open>
		<DropdownMenu.Trigger class="mcp-session-trigger" disabled={sessions.length === 0}>
			{#if selected}
				<span class="session-copy">
					<span class="session-name">{selected.name}</span>
					{#if selected.topic}
						<span class="session-topic">{selected.topic}</span>
					{/if}
				</span>
				{#if selected.pending > 0}
					<span class="pending-dot" title="{selected.pending} waiting">{selected.pending}</span>
				{/if}
			{:else}
				<span class="session-copy">
					<span class="session-name muted">No sessions</span>
					<span class="session-topic">Waiting for a client</span>
				</span>
			{/if}
			<Icon name="chevron-down" size={12} />
		</DropdownMenu.Trigger>

		<DropdownMenu.Portal>
			<DropdownMenu.Content class="mcp-session-content" align="start" sideOffset={4}>
				{#if sessions.length === 0}
					<div class="empty">No MCP clients connected</div>
				{:else}
					{#each sessions as session (session.id)}
						<DropdownMenu.Item
							class="mcp-session-item {mcpSessionsStore.selectedId === session.id ? 'selected' : ''}"
							onSelect={() => mcpSessionsStore.select(session.id)}
						>
							<span class="session-copy">
								<span class="session-name">{session.name}</span>
								{#if session.topic}
									<span class="session-topic">{session.topic}</span>
								{:else}
									<span class="session-topic muted">no topic</span>
								{/if}
							</span>
							{#if session.pending > 0}
								<span class="pending-dot">{session.pending}</span>
							{/if}
							{#if mcpSessionsStore.selectedId === session.id}
								<span class="check-icon">
									<Icon name="check" size={14} strokeWidth={2.5} />
								</span>
							{/if}
						</DropdownMenu.Item>
					{/each}
				{/if}
			</DropdownMenu.Content>
		</DropdownMenu.Portal>
	</DropdownMenu.Root>
</div>

<style>
	.mcp-session-picker {
		flex-shrink: 0;
	}

	:global(.mcp-session-trigger) {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		width: 100%;
		padding: 0.45rem 0.7rem;
		background: var(--bg-secondary);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-full);
		cursor: pointer;
		font-family: inherit;
		color: var(--text-primary);
		text-align: left;
		box-shadow: var(--shadow-md);
		transition: background 0.15s ease, box-shadow 0.15s ease;
	}

	.compact :global(.mcp-session-trigger) {
		height: 56px;
		width: auto;
		max-width: 9.5rem;
		padding: 0.35rem 0.7rem 0.35rem 0.85rem;
	}

	:global(.mcp-session-trigger:hover:not(:disabled)) {
		box-shadow: var(--shadow-lg);
	}

	:global(.mcp-session-trigger:disabled) {
		opacity: 0.7;
		cursor: default;
	}

	:global(.mcp-session-trigger:focus-visible),
	:global(.mcp-session-trigger[data-state='open']) {
		outline: none;
		border-color: var(--accent);
		box-shadow: 0 0 0 3px var(--accent-muted);
	}

	.session-copy {
		display: flex;
		flex-direction: column;
		min-width: 0;
		flex: 1;
		line-height: 1.15;
	}

	.session-name {
		font-size: 0.82rem;
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.session-name.muted {
		color: var(--text-secondary);
		font-weight: 500;
	}

	.session-topic {
		font-size: 0.62rem;
		font-weight: 500;
		color: var(--text-tertiary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.session-topic.muted {
		font-style: italic;
	}

	.pending-dot {
		flex-shrink: 0;
		min-width: 1.1rem;
		height: 1.1rem;
		padding: 0 0.25rem;
		border-radius: var(--radius-full);
		background: var(--accent);
		color: #fff;
		font-size: 0.62rem;
		font-weight: 700;
		display: inline-flex;
		align-items: center;
		justify-content: center;
	}

	:global(.mcp-session-content) {
		z-index: 1050;
		min-width: 180px;
		max-width: 260px;
		background: var(--bg-primary);
		border-radius: var(--radius-lg);
		padding: 0.375rem;
		box-shadow: var(--shadow-lg);
	}

	:global(.mcp-session-item) {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		width: 100%;
		padding: 0.45rem 0.55rem;
		border-radius: var(--radius-sm);
		cursor: pointer;
		outline: none;
	}

	:global(.mcp-session-item:hover),
	:global(.mcp-session-item[data-highlighted]) {
		background: var(--bg-secondary);
	}

	:global(.mcp-session-item.selected) {
		background: var(--accent-muted);
	}

	.check-icon {
		display: flex;
		align-items: center;
		color: var(--accent);
		margin-left: auto;
	}

	.empty {
		padding: 0.7rem;
		text-align: center;
		font-size: 0.78rem;
		color: var(--text-tertiary);
	}
</style>
