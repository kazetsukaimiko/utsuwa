/** The user's own avatar. Extra scene instances must not use this id. */
export const PRIMARY_INSTANCE_ID = 'primary';

export interface VrmInstancePose {
	x: number;
	y: number;
	z: number;
}

export interface VrmExtraInstance {
	id: string;
	modelId: string | null;
	url: string | null;
	position: VrmInstancePose;
}

export interface VrmSceneInstance extends VrmExtraInstance {
	isPrimary: boolean;
}

export const DEFAULT_INSTANCE_SPACING = 1.2;

export function sceneInstances(
	primary: { modelId: string | null; url: string | null },
	extras: VrmExtraInstance[]
): VrmSceneInstance[] {
	return [
		{
			id: PRIMARY_INSTANCE_ID,
			modelId: primary.modelId,
			url: primary.url,
			position: { x: 0, y: 0, z: 0 },
			isPrimary: true
		},
		...extras.map((extra) => ({ ...extra, isPrimary: false }))
	];
}

export function defaultPositionForSlot(
	index: number,
	spacing = DEFAULT_INSTANCE_SPACING
): VrmInstancePose {
	const n = index + 1;
	const x = Math.ceil(n / 2) * spacing * (n % 2 === 1 ? 1 : -1);
	return { x, y: 0, z: 0 };
}

/** Line extras up on X, centered on the origin (one session sits at 0). */
export function centeredSlot(
	index: number,
	count: number,
	spacing = DEFAULT_INSTANCE_SPACING
): VrmInstancePose {
	if (count <= 1) return { x: 0, y: 0, z: 0 };
	const origin = (count - 1) / 2;
	return { x: (index - origin) * spacing, y: 0, z: 0 };
}

export function upsertExtraInstance(
	extras: VrmExtraInstance[],
	inst: VrmExtraInstance
): VrmExtraInstance[] {
	if (!inst.id || inst.id === PRIMARY_INSTANCE_ID) return extras;
	const i = extras.findIndex((e) => e.id === inst.id);
	if (i === -1) return [...extras, inst];
	const next = extras.slice();
	next[i] = { ...next[i], ...inst, id: inst.id };
	return next;
}

export function removeExtraInstance(extras: VrmExtraInstance[], id: string): VrmExtraInstance[] {
	if (id === PRIMARY_INSTANCE_ID) return extras;
	return extras.filter((e) => e.id !== id);
}

export function isPrimaryInstance(id: string | undefined | null): boolean {
	return !id || id === PRIMARY_INSTANCE_ID;
}

/**
 * Mild yaw so a character at `x` looks toward the group center, not outward.
 * Right of center (x > 0) turns left (negative Y). Softened so it is not a 90° profile.
 */
export function inwardFacingYaw(x: number, soften = 0.32): number {
	if (!x) return 0;
	return -Math.atan(x / DEFAULT_INSTANCE_SPACING) * soften;
}

/** World-space gap between instance centers: scale 1 = one character width. */
export function slotSpacingFromScale(scale: number, characterWidth: number): number {
	return Math.max(0, scale) * clampCharacterWidth(characterWidth);
}

/** Rest-pose body width used as the 1.0 mark on the spacing slider. */
export function clampCharacterWidth(width: number): number {
	if (!Number.isFinite(width) || width <= 0) return 0.7;
	return Math.min(1.4, Math.max(0.4, width));
}
