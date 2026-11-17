import assert from 'node:assert/strict';
import test from 'node:test';
import {readHoldSetupStages} from './hold-owned-primitive-worker.mjs';

test('setup metadata preserves only fixed stage vocabulary across stderr fragments',()=>{
  const parts=['DIYIME_HOLD_SETUP python-en','tered\r\nDIYIME_HOLD_SETUP native-api-ready\n',
    'Traceback: private path / input must not become metadata\n',
    'DIYIME_HOLD_SETUP arbitrary-input\nDIYIME_HOLD_SETUP image-verified\n'];
  assert.deepEqual(readHoldSetupStages(parts.join('')),
    ['python-entered','native-api-ready','image-verified']);
});

test('unknown or absent evidence stays absent; repeated messages remain bounded',()=>{
  assert.deepEqual(readHoldSetupStages('Traceback\n'),[]);
  assert.deepEqual(readHoldSetupStages('DIYIME_HOLD_SETUP image-verified\n'.repeat(1000)),
    ['image-verified']);
  assert.deepEqual(readHoldSetupStages('prefix DIYIME_HOLD_SETUP suspension-confirmed\n'),[]);
});
