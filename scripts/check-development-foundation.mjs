import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// 受け入れ例と文書の整合だけを検査する。未実装ノードの動作テストではない。
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
function packet(value) {
  assert(object(value)); assert.equal(value.version, 1);
  function visit(candidate, depth = 1) {
    assert(depth <= 32); assert(object(candidate));
    if (candidate.type === 'text') {
      assert.equal(typeof candidate.text, 'string');
      if (candidate.meta !== undefined) {
        assert(object(candidate.meta));
        if (candidate.meta.reading !== undefined) assert.equal(typeof candidate.meta.reading, 'string');
        if (candidate.meta.sourceSpan !== undefined) {
          const span = candidate.meta.sourceSpan; assert(Array.isArray(span)); assert.equal(span.length, 2);
          assert(span.every(item => Number.isSafeInteger(item) && item >= 0)); assert(span[0] <= span[1]);
        }
      }
    } else if (candidate.type === 'sequence') {
      assert(Array.isArray(candidate.items)); candidate.items.forEach(item => visit(item, depth + 1));
    } else {
      assert.equal(candidate.type, 'choice'); assert(Array.isArray(candidate.options));
      candidate.options.forEach(option => {
        assert(object(option));
        if (option.score !== undefined) assert(Number.isFinite(option.score));
        visit(option.value, depth + 1);
      });
    }
  }
  visit(value.root);
}

let count = 0;
for (const name of ['romaji', 'candidate_selection', 'runtime_input', 'stringify']) {
  const fixture = JSON.parse(readFileSync(resolve(root, `tests/contracts/${name}.json`), 'utf8'));
  assert.equal(fixture.contractVersion, '0.1'); assert.equal(typeof fixture.nodeKind, 'string');
  assert(object(fixture.defaultConfig)); assert(Array.isArray(fixture.cases) && fixture.cases.length > 0);
  const ids = new Set();
  for (const test of fixture.cases) {
    assert.equal(typeof test.id, 'string'); assert(!ids.has(test.id)); ids.add(test.id);
    assert(object(test.expected));
    assert(['data', 'no-output', 'error', 'request-error'].includes(test.expected.status));
    if (test.expected.status.endsWith('error')) assert.equal(typeof test.expected.code, 'string');
    if (test.packet && test.expected.code !== 'invalid-packet') packet(test.packet);
    if (test.expected.packet) packet(test.expected.packet);
    if (name === 'romaji' && test.expected.status === 'data') {
      assert(['compose', 'finalize'].includes(test.mode)); assert.equal(typeof test.text, 'string');
      assert.equal(typeof test.expected.text, 'string'); assert.equal(typeof test.expected.pending, 'string');
      if (test.mode === 'finalize') assert.equal(test.expected.pending, '');
    }
    if (name === 'runtime_input' && test.expected.status === 'data') {
      assert(object(test.input)); assert(Number.isSafeInteger(test.input.revision) && test.input.revision > 0);
      assert(['compose', 'finalize'].includes(test.input.mode)); assert.equal(typeof test.input.text, 'string');
      assert.equal(test.expected.inputRevision, test.input.revision);
      assert.equal(test.expected.packet.root.text, test.input.text);
    }
    count++;
  }
}

const docs = ['AGENTS.md', 'CLAUDE.md', 'docs/parallel-development.md', 'docs/node-contracts-v0.1.md', 'docs/runtime-input-v0.1.md', 'tests/contracts/README.md'];
for (const name of docs) {
  const file = resolve(root, name), content = readFileSync(file, 'utf8');
  for (const link of content.matchAll(/\]\(([^)]+)\)/g)) {
    if (/^(https?:|#)/.test(link[1])) continue;
    const target = resolve(dirname(file), link[1].split('#')[0]);
    assert(existsSync(target), `${name}: missing link ${link[1]}`);
  }
}
for (const name of ['task', 'bug']) {
  const content = readFileSync(resolve(root, `.github/ISSUE_TEMPLATE/${name}.md`), 'utf8');
  assert(/^---\r?\n[\s\S]+?\r?\n---\r?\n/.test(content));
  for (const key of ['name', 'about', 'title', 'labels', 'assignees']) assert(new RegExp(`^${key}:`, 'm').test(content));
}
assert(existsSync(resolve(root, '.github/pull_request_template.md')));
console.log(`契約例 ${count} 件、文書リンク、Issue/PRテンプレートの構造を確認しました。`);
console.log('これは構造検査のみです。実行時入力の動作は cargo test --test runtime_input 等で確認します。');
