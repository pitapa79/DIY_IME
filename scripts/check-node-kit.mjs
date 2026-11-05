import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createNodeKit } from './new-node-kit.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const temporary = mkdtempSync(join(tmpdir(), 'diy-ime2-node-kit-'));
try {
  for (const name of [undefined, '', '../escape', 'a/b', 'a\\b', 'Upper', 'a-b', '_a', 'a__b', 'a'.repeat(49)]) {
    assert.throws(() => createNodeKit(name, temporary), /name must/);
  }
  assert.deepEqual(readdirSync(temporary), []);
  const paths = createNodeKit('kit_smoke', temporary);
  assert.equal(paths.length, 4);
  for (const path of paths) {
    const content = readFileSync(resolve(temporary, path), 'utf8');
    assert(!content.includes('__NAME__') && !content.includes('__CONTRACT__'));
  }
  const generatedTest = readFileSync(resolve(temporary, paths[1]), 'utf8');
  assert(generatedTest.includes('#[path = "../src/nodes/kit_smoke/mod.rs"]'));
  assert(generatedTest.includes('include_str!("../examples/nodes/kit_smoke/contract.json")'));
  assert(!existsSync(resolve(temporary, 'src/nodes/mod.rs')));
  assert(!existsSync(resolve(temporary, 'src/builtins.rs')));
  const fixture = JSON.parse(readFileSync(resolve(temporary, paths[2]), 'utf8'));
  assert.equal(fixture.nodeKind, 'kit.kit_smoke');
  assert.equal(fixture.purpose, 'kit-validation-only');
  assert.equal(fixture.cases.length, 7);
  assert.equal(new Set(fixture.cases.map(test => test.id)).size, fixture.cases.length);
  const before = paths.map(path => readFileSync(resolve(temporary, path), 'utf8'));
  assert.throws(() => createNodeKit('kit_smoke', temporary), /already exists/);
  assert.deepEqual(paths.map(path => readFileSync(resolve(temporary, path), 'utf8')), before);
  for (const [index, occupied] of [
    'src/nodes/occupied', 'examples/nodes/occupied', 'tests/nodes_occupied.rs',
  ].entries()) {
    const destination = resolve(temporary, 'conflict' + index);
    const target = resolve(destination, occupied);
    mkdirSync(dirname(target), { recursive: true });
    if (occupied.endsWith('.rs')) writeFileSync(target, 'sentinel');
    else mkdirSync(target);
    assert.throws(() => createNodeKit('occupied', destination), /already exists/);
    assert.equal(existsSync(resolve(destination, 'src/nodes/occupied/mod.rs')), false);
    if (occupied.endsWith('.rs')) assert.equal(readFileSync(target, 'utf8'), 'sentinel');
  }
} finally {
  rmSync(temporary, { recursive: true, force: true });
}
for (const name of ['AGENTS.md', 'CLAUDE.md', 'docs/parallel-development.md', 'docs/lightweight-node-kit.md', 'dev/node-kit/claude-task.md']) {
  const path = resolve(root, name);
  for (const [, link] of readFileSync(path, 'utf8').matchAll(/\]\(([^)]+)\)/g)) {
    if (/^(https?:|#)/.test(link)) continue;
    assert(existsSync(resolve(dirname(path), link.split('#')[0])), name + ': missing ' + link);
  }
}
console.log('Node kit: generation, collision refusal, fixture and documentation links passed.');
console.log('Runtime verification: cargo test --locked --test node_kit');
