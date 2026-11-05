import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export function createNodeKit(name, destination = root) {
  if (!/^[a-z][a-z0-9]*(?:_[a-z0-9]+)*$/.test(name ?? '') || name.length > 48) {
    throw new Error('name must be lower snake_case (1-48 characters)');
  }
  const template = resolve(root, 'dev/node-kit/template');
  const read = file => readFileSync(resolve(template, file), 'utf8');
  const render = text => text.replaceAll('__NAME__', name);
  const outputs = [
    [ 'src/nodes/' + name + '/mod.rs', render(read('mod.rs')) ],
    [ 'tests/nodes_' + name + '.rs',
      '#[path = "../src/nodes/' + name + '/mod.rs"]\nmod subject;\n\n' +
      render(read('checks.rs')).replaceAll('contract.json', '../examples/nodes/' + name + '/contract.json') ],
    [ 'examples/nodes/' + name + '/contract.json', render(read('contract.json')) ],
    [ 'examples/nodes/' + name + '/README.md', render(read('README.md')) ],
  ];
  // 既存ノードや途中作業を上書きしない。全候補を確認してから書く。
  for (const path of ['src/nodes/' + name, 'examples/nodes/' + name, 'tests/nodes_' + name + '.rs']) {
    if (existsSync(resolve(destination, path))) throw new Error('already exists: ' + path);
  }
  for (const [path, content] of outputs) {
    const target = resolve(destination, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, content, { encoding: 'utf8', flag: 'wx' });
  }
  return outputs.map(([path]) => path);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 3) throw new Error('usage: node scripts/new-node-kit.mjs <name>');
    console.log(createNodeKit(process.argv[2]).join('\n'));
    console.log('Kit validation only. Standard Registry is unchanged.');
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
