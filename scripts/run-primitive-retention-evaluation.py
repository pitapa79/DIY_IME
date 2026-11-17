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
parser.add_argument('suite', choices=['comparison', 'safety', 'recovery', 'timing', 'startup'])
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
env['DIY_EXPERIMENT_HOST_TRACE_LOSS_AUDIT'] = '1'
env['DIY_PRIMITIVE_SAMPLE_DELIVERY_CHECK'] = '1'
cases = []
timing_controls = []
startup_limits = []
if args.suite == 'comparison':
    for count in [1, 4]:
        cases.extend((mode, count, 3, 'identity', 'warm') for mode in ['0', 'separate', 'grouped'])
    for shape, samples in [('identity', 30), ('mixed', 20)]:
        cases.extend((mode, 20, samples, shape, 'warm')
                     for mode in ['0', 'separate', 'grouped', 'grouped', 'separate', '0'])
elif args.suite == 'safety':
    for mode in ['separate', 'grouped']:
        cases.append((mode, 20, 1, 'identity', 'safety'))
        cases.append((mode, 20, 5, 'mixed', 'paced'))
elif args.suite == 'recovery':
    # Four adjacent trials per condition, raw/compiled/compiled/raw.
    for count in [1, 4, 20]:
        for shape in (['identity', 'mixed'] if count == 20 else ['identity']):
            for cadence, samples in [('warm', 30), ('paced', 5)]:
                cases.extend(('separate', count, samples, shape, cadence) for _ in range(4))
    cases.extend(('separate', 20, 1, 'identity', 'safety') for _ in range(4))
elif args.suite == 'timing':
    def timing_cases(count, samples, shape, cadence, compiled_modes=(False, True)):
        for compiled in compiled_modes:
            for measured in [False, True, True, False]:
                cases.append(('separate', count, samples, shape, cadence))
                timing_controls.append((compiled, measured))
    for count in [1, 4, 20]:
        timing_cases(count, 30, 'identity', 'warm')
    timing_cases(20, 5, 'mixed', 'paced')
    timing_cases(20, 5, 'identity', 'cold-paced')
    timing_cases(20, 1, 'identity', 'cold', (True,))
    for compiled in [False, True]:
        cases.append(('separate', 20, 1, 'identity', 'safety'))
        timing_controls.append((compiled, True))
    assert len(cases) == len(timing_controls) == 46
else:
    # Verify refusal before longer comparisons can fail on diagnostic loss.
    for limit in [2, 4]:
        cases.append(('separate', 1, 1, 'identity', 'startup-refusal'))
        startup_limits.append(limit)
    for count in [1, 4, 20]:
        for limit in [1, 2, 4]:
            cases.append(('separate', count, 1, 'identity', 'cold'))
            startup_limits.append(limit)
    for limit in [1, 2, 4]:
        cases.append(('separate', 20, 5, 'identity', 'cold-paced'))
        startup_limits.append(limit)
    for limit in [2, 4]:
        cases.append(('separate', 20, 1, 'identity', 'safety'))
        startup_limits.append(limit)
    assert len(cases) == len(startup_limits) == 16

manifest = {'source': os.environ.get('GITHUB_SHA'), 'suite': args.suite,
            'syntheticOnly': True, 'cases': [], 'complete': False}
def save():
    files = [*binaries, official/'official.wasm', Path(node),
             *sorted((repo/'scripts').glob('*primitive*'))]
    files.extend(sorted((repo/'scripts').glob('analyze-retained-*.mjs')))
    files.extend([repo/'scripts/host-input-scenarios.mjs', repo/'scripts/host-input-intervals.test.mjs',
                  repo/'scripts/private-sample-delivery.mjs', repo/'scripts/private-sample-delivery.test.mjs',
                  repo/'scripts/terminate-owned-retention-worker.py', repo/'editor/src/executionTrace.ts'])
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
        compiled, measured = timing_controls[index] if args.suite == 'timing' else (
            args.suite == 'startup' or args.suite == 'recovery' and index % 4 in [1, 2], args.suite == 'startup')
        startup_limit = startup_limits[index] if args.suite == 'startup' else 1
        env['DIY_EXPERIMENT_RETAINED_STARTUP_PARALLELISM'] = str(startup_limit)
        env['DIY_EXPERIMENT_RETAINED_COMPILED_JIT'] = '1' if compiled else '0'
        env['DIY_EXPERIMENT_RETAINED_TIMING'] = '1' if measured else '0'
        if cadence == 'startup-refusal':
            env['DIY_EXPERIMENT_COMPILED_JIT'] = '0'
        else:
            env['DIY_EXPERIMENT_COMPILED_JIT'] = '1'
        env['DIY_PRIMITIVE_COMPARE_TIMING'] = '1' if args.suite in ['timing', 'startup'] else '0'
        env['DIY_PRIMITIVE_RETENTION_REPORT'] = str(output/(name+'.json'))
        command = [sys.executable, 'scripts/measure-primitive-retention-job.py', '--command',
                   str(output/(name+'.job.json')), node, 'scripts/benchmark-primitive-retention.mjs',
                   *map(str, binaries), str(official), str(count), str(samples), shape, cadence]
        case = dict(name=name, mode=mode, count=count, samples=samples, shape=shape, cadence=cadence,
                    retainedCompiled=compiled, retainedTiming=measured, startupParallelism=startup_limit)
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
