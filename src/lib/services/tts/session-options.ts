import { modulesStore } from '$lib/stores/modules.svelte';
import { settingsStore } from '$lib/stores/settings.svelte';
import { getTTSProvider } from '$lib/services/providers/registry';
import type { TTSProvider } from '$lib/types';
import type { TTSOptions } from './index.ts';

/** Snapshot of the speech module used by chat and the MCP puppet bridge. */
export function isSpeechEnabled(): boolean {
	return modulesStore.getModuleState('speech')?.enabled === true;
}

export function getCurrentTtsOptions(): TTSOptions | null {
	const settings = modulesStore.getModuleSettings('speech');
	const provider = settings.activeProvider as TTSProvider;
	if (!provider) return null;

	const ttsConfig = settingsStore.getProviderConfig(provider);
	const ttsMeta = getTTSProvider(provider);
	const base: TTSOptions = {
		provider,
		apiKey: ttsConfig.apiKey,
		voiceId: (settings.activeVoiceId as string) || undefined,
		model: (settings.activeModel as string) || ttsConfig.modelId,
		baseUrl: ttsConfig.baseUrl || ttsMeta?.defaultBaseUrl,
		speed: (settings.speed as number) ?? 1,
		language: (settings.activeLanguage as string) || undefined,
		altLanguage: (settings.altLanguage as string) || undefined,
		altVoiceId: (settings.altVoiceId as string) || undefined,
		enableAltLanguage: (settings.enableAltLanguage as boolean) ?? false,
		altSpeed: (settings.altSpeed as number) ?? undefined
	};

	if (provider !== 'omnivoice') return base;

	return {
		...base,
		instructions: (settings.instructions as string) || undefined,
		altInstructions: (settings.altInstructions as string) || undefined,
		numStep: (settings.numStep as number) ?? undefined,
		altNumStep: (settings.altNumStep as number) ?? undefined,
		positionTemperature: (settings.positionTemperature as number) ?? undefined,
		classTemperature: (settings.classTemperature as number) ?? undefined,
		altPositionTemperature: (settings.altPositionTemperature as number) ?? undefined,
		altClassTemperature: (settings.altClassTemperature as number) ?? undefined
	};
}
