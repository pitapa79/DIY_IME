"""Bounded real-Host S1 composition/input evidence; no product acceptance claims."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(65536), b''):
            digest.update(block)
    return digest.hexdigest()


def read(path):
    return json.loads(path.read_text(encoding='utf-8-sig'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('preflight', type=Path)
    args = parser.parse_args()
    assert sys.platform == 'win32'
    repo = Path(__file__).resolve().parent.parent
    bundle, output = args.bundle.resolve(strict=True), args.output.resolve()
    preflight = args.preflight.resolve(strict=True)
    assert output.is_relative_to(repo/'target') and not output.exists()
    source = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
    pin = read(bundle/'bundle-manifest.json')
    assert pin['sourceCommit'] == source
    files = ['bin/diy-ime2-external-host.exe', 'bin/diy-ime2-node-manager.exe',
             'bin/diy-ime2-wasm-worker.exe', 'runtime/wasmtime.dll', 'runtime/node.exe',
             'official/official.wasm', 'official/demo.identity.manifest.json']
    hashes = {entry['path']: entry['sha256'] for entry in pin['files']}
    assert all(sha(bundle/path) == hashes[path] for path in files)
    modules = [repo/'target/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm',
               repo/'target/alternate-wasm/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm']
    assert all(path.is_file() for path in modules)
    output.mkdir(parents=True)
    planned = output/'planned-cases.json'
    subprocess.run([str(bundle/'runtime/node.exe'), 'scripts/retained-composition-preflight.mjs',
                    str(bundle/'official'), str(planned)], cwd=repo, check=True)
    cases = read(planned)
    assert len(cases) == 25
    assert {c['layout'] for c in cases} == {'serial', 'parallel', 'join', 'unused'}
    assert {c['count'] for c in cases if c['mode'] == 'separate' and c['layout'] == 'serial'} == {1, 2, 4, 8, 16, 20}
    manifest = {'issue': 135, 'parentGoal': 109, 'sourceCommit': source,
                'scope': 'Synthetic normalHost topology/input coverage; guest spans include SDK ABI. No semantic/framework target or product adoption claim.',
                'bundleFiles': {path: hashes[path] for path in files},
                'moduleFiles': {str(path.relative_to(repo)): sha(path) for path in modules},
                'preflightExeSha256': sha(preflight),
                'harnessFilesSha256': {name: sha(repo/name) for name in [
                    'scripts/run-retained-composition-matrix.py', 'scripts/retained-composition-cases.mjs',
                    'scripts/retained-composition-preflight.mjs', 'scripts/retained-composition-graphs.mjs',
                    'scripts/analyze-retained-graph-calls.mjs', 'scripts/benchmark-primitive-retention.mjs',
                    'scripts/host-input-scenarios.mjs', 'scripts/host-warm-input-observation.mjs', 'scripts/analyze-retained-call-timing.mjs',
                    'scripts/private-sample-delivery.mjs', 'scripts/measure-primitive-retention-job.py',
                    'editor/src/executionTrace.ts']}, 'plannedCases': cases,
                'cases': [], 'complete': False, 'notRun': []}
    def save():
        manifest['evidenceFiles'] = {path.name: sha(path) for path in output.iterdir()
                                   if path.is_file() and path.name != 'manifest.json'}
        manifest['notRun'] = [{'name': c['name'], 'reason': 'not yet executed or stopped after earlier failure'}
                              for c in cases[len(manifest['cases']):]]
        (output/'manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
    base = {key: value for key, value in os.environ.items() if not key.startswith('DIY_EXPERIMENT_')}
    base.pop('GITHUB_SHA', None)
    base.update({'DIY_REVIEW_MODULE_A': str(modules[0]), 'DIY_REVIEW_MODULE_B': str(modules[1]),
                 'DIY_PRIMITIVE_MATRIX': '1', 'DIY_PRIMITIVE_SAMPLE_DELIVERY_CHECK': '1',
                 'DIY_PRIMITIVE_PYTHON': sys.executable, 'DIY_MEASUREMENT_SOURCE_SHA': source,
                 'DIY_MEASUREMENT_BUILD_SOURCE_SHA': source})
    try:
        save()
        # Real Core type/port compile before ANY expensive native case. Isolated
        # config/ABI/package validation remains mandatory in every actual Host.
        with (output/'preflight.log').open('x', encoding='utf-8') as log:
            result = subprocess.run([str(preflight), str(planned)], cwd=repo, stdout=log, stderr=log)
        manifest['preflightExitCode'] = result.returncode
        save()
        assert result.returncode == 0, 'Core graph preflight failed; no native case ran'
        for index, spec in enumerate(cases):
            name = f'{index:02}-{spec["mode"]}-{spec["count"]}-{spec["layout"]}-{spec["cadence"]}-timing{int(spec["timing"])}'
            report, job = output/(name+'.json'), output/(name+'.job.json')
            env = base.copy()
            env.update({'DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION': spec['mode'],
                        'DIY_EXPERIMENT_RETAINED_TIMING': str(int(spec['timing'])),
                        'DIY_PRIMITIVE_LAYOUT': spec['layout'],
                        'DIY_PRIMITIVE_RETENTION_REPORT': str(report)})
            if spec['mode'] == 'separate':
                for flag in ['COMPILED_JIT', 'INHERITED_CODE_ACL', 'VERIFIED_RUNTIME_FILE',
                             'OVERLAP_WORKER_CLEANUP', 'NEUTRAL_CODE_CACHE', 'NEUTRAL_CODE_MAPPING']:
                    env['DIY_EXPERIMENT_'+flag] = '1'
                env['DIY_EXPERIMENT_RETAINED_COMPILED_JIT'] = '1'
                env['DIY_EXPERIMENT_RETAINED_STARTUP_PARALLELISM'] = '2'
            command = list(map(str, [sys.executable, repo/'scripts/measure-primitive-retention-job.py',
                '--command', job, bundle/'runtime/node.exe', repo/'scripts/benchmark-primitive-retention.mjs',
                bundle/files[0], bundle/files[1], bundle/files[2], bundle/files[3], bundle/'official',
                spec['count'], spec['samples'], 'thirdparty', spec['cadence']]))
            case = {'name': name, 'plannedName': spec['name'], 'spec': spec, 'command': command,
                    'runtimeFlags': {k: v for k, v in env.items() if k.startswith('DIY_EXPERIMENT_')}}
            manifest['cases'].append(case)
            save()
            print('Starting '+name, flush=True)
            with (output/(name+'.log')).open('x', encoding='utf-8') as log:
                result = subprocess.run(command, cwd=repo, env=env, stdout=log, stderr=log, timeout=330)
            case['exitCode'] = result.returncode
            save()
            assert result.returncode == 0, 'Failed native case preserved; no retry'
            data, owned = read(report), read(job)
            assert data['completed'] and data['sourceCommit'] == source and data['runtimeBuildSourceCommit'] == source and data['workspaceAfterHostExit'] == []
            assert data['binaries']['host'] == hashes[files[0]]
            assert all(t['dropped'] == 0 for t in data['traceCollections'])
            assert owned['driverExitCode'] == 0 and owned['allDescendantsExited'] and not owned['timedOut']
            assert owned['missingProcessMemoryReads'] == 0
            case.update(checksPassed=True, resourcePolicy=data['resourcePolicy'],
                        executedIds=data['executedIds'], unexecutedIds=data['unexecutedIds'],
                        warm=data.get('warmResultMs'), pacing=data.get('pacing'))
            save()
        manifest['complete'] = True
    except BaseException as error:
        manifest['failure'] = {'type': type(error).__name__, 'message': str(error)}
        raise
    finally:
        save()


if __name__ == '__main__':
    main()
