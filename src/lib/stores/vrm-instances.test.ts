import test from 'node:test';
import assert from 'node:assert/strict';
import {
	PRIMARY_INSTANCE_ID,
	sceneInstances,
	defaultPositionForSlot,
	upsertExtraInstance,
	removeExtraInstance,
	isPrimaryInstance,
	inwardFacingYaw,
	slotSpacingFromScale,
	clampCharacterWidth
} from './vrm-instances.ts';

test('sceneInstances always leads with the primary avatar', () => {
	const list = sceneInstances({ modelId: 'a', url: '/a.vrm' }, [
		{ id: 'sess-1', modelId: 'b', url: '/b.vrm', position: { x: 1.2, y: 0, z: 0 } }
	]);
	assert.equal(list.length, 2);
	assert.equal(list[0].id, PRIMARY_INSTANCE_ID);
	assert.equal(list[0].isPrimary, true);
	assert.equal(list[1].id, 'sess-1');
	assert.equal(list[1].isPrimary, false);
});

test('defaultPositionForSlot alternates right then left', () => {
	assert.deepEqual(defaultPositionForSlot(0), { x: 1.2, y: 0, z: 0 });
	assert.deepEqual(defaultPositionForSlot(1), { x: -1.2, y: 0, z: 0 });
	assert.deepEqual(defaultPositionForSlot(2), { x: 2.4, y: 0, z: 0 });
});

test('upsertExtraInstance refuses the primary id', () => {
	assert.deepEqual(
		upsertExtraInstance([], {
			id: PRIMARY_INSTANCE_ID,
			modelId: 'x',
			url: '/x.vrm',
			position: { x: 0, y: 0, z: 0 }
		}),
		[]
	);
});

test('upsertExtraInstance inserts and updates by id', () => {
	let extras = upsertExtraInstance([], {
		id: 'sess-1',
		modelId: 'b',
		url: '/b.vrm',
		position: { x: 1.2, y: 0, z: 0 }
	});
	assert.equal(extras.length, 1);
	extras = upsertExtraInstance(extras, {
		id: 'sess-1',
		modelId: 'c',
		url: '/c.vrm',
		position: { x: 1.2, y: 0, z: 0 }
	});
	assert.equal(extras.length, 1);
	assert.equal(extras[0].modelId, 'c');
});

test('removeExtraInstance cannot drop the primary avatar', () => {
	const extras = [
		{ id: 'sess-1', modelId: 'b', url: '/b.vrm', position: { x: 1.2, y: 0, z: 0 } }
	];
	assert.equal(removeExtraInstance(extras, PRIMARY_INSTANCE_ID).length, 1);
	assert.equal(removeExtraInstance(extras, 'sess-1').length, 0);
});

test('isPrimaryInstance treats missing ids as primary', () => {
	assert.equal(isPrimaryInstance(undefined), true);
	assert.equal(isPrimaryInstance(PRIMARY_INSTANCE_ID), true);
	assert.equal(isPrimaryInstance('sess-1'), false);
});

test('inwardFacingYaw turns side characters toward the center', () => {
	assert.equal(inwardFacingYaw(0), 0);
	assert.ok(inwardFacingYaw(1.2) < 0, 'right side turns left');
	assert.ok(inwardFacingYaw(-1.2) > 0, 'left side turns right');
	assert.ok(Math.abs(inwardFacingYaw(1.2)) < Math.PI / 8, 'stays a mild angle');
});

test('slotSpacingFromScale is character-widths between centers', () => {
	assert.equal(slotSpacingFromScale(1, 0.8), 0.8);
	assert.equal(slotSpacingFromScale(0.5, 0.8), 0.4);
	assert.equal(slotSpacingFromScale(2, 0.8), 1.6);
	assert.equal(slotSpacingFromScale(0, 0.8), 0);
});

test('clampCharacterWidth ignores animated or empty bounds', () => {
	assert.equal(clampCharacterWidth(0.8), 0.8);
	assert.equal(clampCharacterWidth(0), 0.7);
	assert.equal(clampCharacterWidth(Number.NaN), 0.7);
	assert.equal(clampCharacterWidth(40), 1.4);
});
