"""Normal Host S1 qualification with real distinct Wasm and official packages."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(65536), b''):
            digest.update(block)
    return digest.hexdigest()


def read(path):
    return json.loads(path.read_text(encoding='utf-8-sig'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    assert sys.platform == 'win32'
    repo = Path(__file__).resolve().parent.parent
    bundle, output = args.bundle.resolve(strict=True), args.output.resolve()
    assert output.is_relative_to(repo/'target') and not output.exists()
    source = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
    pin = read(bundle/'bundle-manifest.json')
    assert pin['sourceCommit'] == source
    files = ['bin/diy-ime2-external-host.exe', 'bin/diy-ime2-node-manager.exe',
             'bin/diy-ime2-wasm-worker.exe', 'runtime/wasmtime.dll', 'runtime/node.exe']
    hashes = {entry['path']: entry['sha256'] for entry in pin['files']}
    assert all(sha(bundle/path) == hashes[path] for path in files)
    modules = [repo/'target/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm',
               repo/'target/alternate-wasm/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm']
    assert all(path.is_file() for path in modules)
    output.mkdir(parents=True)
    manifest = {'issue': 133, 'parentGoal': 109, 'sourceCommit': source,
                'bundleFiles': {path: hashes[path] for path in files},
                'moduleFiles': {str(path.relative_to(repo)): sha(path) for path in modules},
                'scope': 'Actual distinct executable fixtures plus official identity; correctness/safety smoke, not full Goal109 performance qualification.',
                'cases': [], 'complete': False}
    def save():
        manifest['evidenceFiles'] = {path.name: sha(path) for path in output.iterdir()
                                   if path.is_file() and path.name != 'manifest.json'}
        (output/'manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
    env = {key: value for key, value in os.environ.items() if not key.startswith('DIY_EXPERIMENT_')}
    env.update({'DIY_REVIEW_MODULE_A': str(modules[0]), 'DIY_REVIEW_MODULE_B': str(modules[1]),
                'DIY_EXPERIMENT_RETAINED_TIMING': '1', 'DIY_PRIMITIVE_SAMPLE_DELIVERY_CHECK': '1',
                'DIY_PRIMITIVE_PYTHON': sys.executable, 'DIY_MEASUREMENT_SOURCE_SHA': source,
                'DIY_MEASUREMENT_BUILD_SOURCE_SHA': source})
    try:
        for index, (mode, count, cadence) in enumerate([
            ('0', 1, 'warm'), ('separate', 1, 'warm'), ('separate', 4, 'warm'),
            ('separate', 20, 'warm'), ('separate', 20, 'safety'),
            ('separate', 1, 'call-limit'),
        ]):
            name = f'{index:02}-{mode}-{count}-{cadence}'
            report, job = output/(name+'.json'), output/(name+'.job.json')
            env['DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION'] = mode
            env['DIY_EXPERIMENT_RETAINED_TIMING'] = '0' if mode == '0' else '1'
            env['DIY_EXPERIMENT_RETAINED_COMPILED_JIT'] = '1' if cadence == 'call-limit' else '0'
            env['DIY_EXPERIMENT_COMPILED_JIT'] = env['DIY_EXPERIMENT_RETAINED_COMPILED_JIT']
            env['DIY_PRIMITIVE_RETENTION_REPORT'] = str(report)
            command = list(map(str, [sys.executable, repo/'scripts/measure-primitive-retention-job.py',
                '--command', job, bundle/'runtime/node.exe', repo/'scripts/benchmark-primitive-retention.mjs',
                bundle/files[0], bundle/files[1], bundle/files[2], bundle/files[3], bundle/'official',
                count, 3 if cadence == 'warm' else 1, 'thirdparty', cadence]))
            case = {'name': name, 'command': command}
            manifest['cases'].append(case)
            save()
            with (output/(name+'.log')).open('x', encoding='utf-8') as log:
                result = subprocess.run(command, cwd=repo, env=env, stdout=log, stderr=log, timeout=330)
            case['exitCode'] = result.returncode
            save()
            assert result.returncode == 0, 'Preserve failed case; no retry'
            data, owned = read(report), read(job)
            assert data['completed'] and data['workspaceAfterHostExit'] == []
            assert data['sourceCommit'] == source and data['runtimeBuildSourceCommit'] == source
            assert data['binaries']['host'] == hashes[files[0]]
            assert len(data['reviewedImplementations']) == 2
            assert data['reviewedImplementations'][0]['executableCodeSha256'] != data['reviewedImplementations'][1]['executableCodeSha256']
            assert owned['allDescendantsExited'] and not owned['timedOut']
            assert owned['driverExitCode'] == 0 and owned['missingProcessMemoryReads'] == 0
            assert all(trace['dropped'] == 0 for trace in data['traceCollections'])
            if cadence == 'call-limit':
                proof = data['retainedCallLimit']
                assert proof['passed'] and proof['inputsVerified'] == proof['callBudget'] + 1 == 4097
            case['checksPassed'] = True
            save()
        manifest['complete'] = True
    finally:
        save()


if __name__ == '__main__':
    main()
