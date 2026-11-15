"""Bounded real-Host evidence; default policy and existing CI gates stay intact."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument('bundle', type=Path)
parser.add_argument('output', type=Path)
parser.add_argument('suite', choices=['comparison', 'safety'])
args = parser.parse_args()
repo = Path(__file__).resolve().parent.parent
bundle = args.bundle.resolve(strict=True)
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=False)
node = shutil.which('node')
assert node, 'Node runtime required'
binaries = [bundle/'bin/diy-ime2-external-host.exe', bundle/'bin/diy-ime2-node-manager.exe',
            bundle/'bin/diy-ime2-wasm-worker.exe', bundle/'runtime/wasmtime.dll']
official = bundle/'official'
assert all(p.is_file() for p in binaries) and official.is_dir()
controls = ['INHERITED_CODE_ACL', 'VERIFIED_RUNTIME_FILE', 'OVERLAP_WORKER_CLEANUP',
            'COMPILED_JIT', 'POOLED_JIT', 'JIT_STREAM_MODULE', 'NEUTRAL_JIT_POOL',
            'NEUTRAL_CODE_CACHE', 'NEUTRAL_CODE_MAPPING']
env = {k: v for k, v in os.environ.items() if not k.startswith('DIY_EXPERIMENT_')}
env.update({'DIY_EXPERIMENT_'+k: '1' for k in controls})
env['DIY_PRIMITIVE_PYTHON'] = sys.executable
cases = []
if args.suite == 'comparison':
    for count in [1, 4]:
        cases.extend((mode, count, 3, 'identity', 'warm') for mode in ['0', 'separate', 'grouped'])
    for shape, samples in [('identity', 30), ('mixed', 20)]:
        cases.extend((mode, 20, samples, shape, 'warm')
                     for mode in ['0', 'separate', 'grouped', 'grouped', 'separate', '0'])
else:
    for mode in ['separate', 'grouped']:
        cases.append((mode, 20, 1, 'identity', 'safety'))
        cases.append((mode, 20, 5, 'mixed', 'paced'))

manifest = {'source': os.environ.get('GITHUB_SHA'), 'suite': args.suite,
            'syntheticOnly': True, 'cases': [], 'complete': False}
def save():
    files = [*binaries, official/'official.wasm', Path(node),
             *sorted((repo/'scripts').glob('*primitive*'))]
    files.extend(sorted(output.glob('*.json')))
    files.extend(sorted(output.glob('*.log')))
    manifest['files'] = [{'path': str(p), 'sha256': hashlib.sha256(p.read_bytes()).hexdigest()}
                         for p in files if p.is_file() and p.name != 'manifest.json']
    (output/'manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
try:
    for index, (mode, count, samples, shape, cadence) in enumerate(cases):
        name = f'{index:02}-{mode}-{count}-{shape}-{cadence}'
        env['DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION'] = mode
        env['DIY_EXPERIMENT_PREPARED_JIT'] = 'all' if mode == '0' else '0'
        env['DIY_PRIMITIVE_RETENTION_REPORT'] = str(output/(name+'.json'))
        command = [sys.executable, 'scripts/measure-primitive-retention-job.py', '--command',
                   str(output/(name+'.job.json')), node, 'scripts/benchmark-primitive-retention.mjs',
                   *map(str, binaries), str(official), str(count), str(samples), shape, cadence]
        case = dict(name=name, mode=mode, count=count, samples=samples, shape=shape, cadence=cadence)
        manifest['cases'].append(case)
        save()
        print(name, flush=True)
        with (output/(name+'.log')).open('x', encoding='utf-8') as log:
            result = subprocess.run(command, cwd=repo, env=env, stdout=log, stderr=log, timeout=330)
        case['exitCode'] = result.returncode
        save()
        if result.returncode:
            raise SystemExit(result.returncode)
    manifest['complete'] = True
finally:
    save()
