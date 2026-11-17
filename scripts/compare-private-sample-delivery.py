"""Actual Windows ABBA evidence using a matched reply-behavior control build.

Both builds include queue repair128. No source patching, authority/default change,
synthetic Host, weakened gates, retries or independent percentile subtraction.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

BASELINE_FEATURE = 'private-sample-replies-baseline'


def sha(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(65536), b''):
            value.update(chunk)
    return value.hexdigest()


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()


def read(path):
    return json.loads(path.read_text(encoding='utf-8-sig'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    assert sys.platform == 'win32', 'Actual Windows Worker/Job required'
    repo = Path(__file__).resolve().parent.parent
    bundle = args.bundle.resolve(strict=True)
    output = args.output.resolve()
    assert output.is_relative_to(repo/'target') and not output.exists()
    current = git(repo, 'rev-parse', 'HEAD')
    pin = read(bundle/'bundle-manifest.json')
    assert pin['sourceCommit'] == current

    def rust_sources(folder):
        return {p: sha(folder/p) for p in git(folder, 'ls-files').splitlines()
                if p.endswith('.rs') or Path(p).name in ('Cargo.toml', 'Cargo.lock')}

    sources = rust_sources(repo)
    used = ['bin/diy-ime2-external-host.exe', 'bin/diy-ime2-node-manager.exe',
            'bin/diy-ime2-wasm-worker.exe', 'runtime/wasmtime.dll',
            'runtime/node.exe', 'official/official.wasm']
    pinned_files = {entry['path']: entry['sha256'] for entry in pin['files']}
    assert all(sha(bundle/p) == pinned_files[p] for p in used)
    output.mkdir(parents=True, exist_ok=False)
    manifest = {'issue': 127, 'parentGoal': 109, 'source': current, 'baselineSource': current,
                'buildFeatures': {'old': [BASELINE_FEATURE], 'new': []}, 'sourceRustSha256': sources,
                'comparisonDefinition': 'old=matched current source with previous ordinary reply sample behavior; new=default current distribution build',
                'bundleFilesSha256': {p: pinned_files[p] for p in used},
                'componentSources': {'oldHost': current, 'newHost': current,
                                     'manager': current, 'worker': current},
                'harnessFilesSha256': {p: sha(repo/p) for p in [
                    'scripts/compare-private-sample-delivery.py', 'scripts/benchmark-primitive-retention.mjs',
                    'scripts/private-sample-delivery.mjs', 'scripts/host-input-scenarios.mjs',
                    'scripts/observed-native-ring-history.mjs',
                    'scripts/retained-composition-graphs.mjs', 'scripts/analyze-retained-graph-calls.mjs',
                    'scripts/host-warm-input-observation.mjs',
                    'scripts/analyze-retained-call-timing.mjs', 'scripts/analyze-retained-lifecycle.mjs',
                    'scripts/measure-primitive-retention-job.py', 'editor/src/executionTrace.ts']},
                'scope': 'Same-runner same-release old/new ABBA; both include queue fix128; synthetic20-node fixture, not semantic Node work or Goal109 completion.',
                'cases': [], 'complete': False}

    def save():
        manifest['evidenceFilesSha256'] = {p.name: sha(p) for p in output.iterdir()
                                         if p.is_file() and p.name != 'manifest.json'}
        (output/'manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')

    try:
        env = os.environ.copy()
        cargo_home = Path(env.get('CARGO_HOME') or str(Path.home()/'.cargo'))
        flags = ['--remap-path-prefix='+str(repo)+'=source',
                 '--remap-path-prefix='+str(cargo_home/'registry')+'=registry']
        env.pop('RUSTFLAGS', None)
        env['CARGO_ENCODED_RUSTFLAGS'] = chr(31).join(flags)
        target = repo/'target/private-sample-baseline-build'
        env['CARGO_TARGET_DIR'] = str(target)
        env['CARGO_INCREMENTAL'] = '0'
        command = ['cargo', 'build', '--locked', '--offline', '--release', '--bin', 'diy-ime2-external-host',
                   '--features', BASELINE_FEATURE]
        manifest['baselineBuild'] = {'command': command, 'rustflags': flags,
                                    'rustc': subprocess.check_output(['rustc', '-Vv'], text=True)}
        save()
        build_started = time.monotonic()
        with (output/'baseline-build.log').open('x', encoding='utf-8') as log:
            result = subprocess.run(command, cwd=repo, env=env, stdout=log, stderr=log)
        manifest['baselineBuild']['exitCode'] = result.returncode
        manifest['baselineBuild']['seconds'] = time.monotonic()-build_started
        save()
        assert result.returncode == 0, 'Baseline compilation failed; no native comparison ran'
        assert rust_sources(repo) == sources
        old_host = output/'baseline-host.exe'
        shutil.copyfile(target/'release/diy-ime2-external-host.exe', old_host)
        assert sha(old_host) == sha(target/'release/diy-ime2-external-host.exe')
        manifest['baselineHostSha256'] = sha(old_host)
        assert manifest['baselineHostSha256'] != pinned_files[used[0]], 'Control must be a distinct executable'
        hosts = {'old': old_host, 'new': bundle/'bin/diy-ime2-external-host.exe'}
        native_env = {k: v for k, v in os.environ.items() if not k.startswith('DIY_EXPERIMENT_')}
        native_env.update({'DIY_PRIMITIVE_SAMPLE_DELIVERY_OBSERVE': '1', 'DIY_PRIMITIVE_SAMPLE_DELIVERY_CHECK': '0',
                           'DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION': 'separate',
                           'DIY_EXPERIMENT_RETAINED_COMPILED_JIT': '1',
                           'DIY_EXPERIMENT_RETAINED_STARTUP_PARALLELISM': '2',
                           'DIY_EXPERIMENT_HOST_TRACE_LOSS_AUDIT': '1',
                           'DIY_EXPERIMENT_PREPARED_JIT': '0', 'DIY_PRIMITIVE_COMPARE_TIMING': '1',
                           'DIY_PRIMITIVE_PYTHON': sys.executable})
        for key in ['INHERITED_CODE_ACL', 'VERIFIED_RUNTIME_FILE', 'OVERLAP_WORKER_CLEANUP', 'COMPILED_JIT',
                    'POOLED_JIT', 'JIT_STREAM_MODULE', 'NEUTRAL_JIT_POOL', 'NEUTRAL_CODE_CACHE', 'NEUTRAL_CODE_MAPPING']:
            native_env['DIY_EXPERIMENT_'+key] = '1'
        native_env.pop('GITHUB_SHA', None)
        native_env['DIY_MEASUREMENT_SOURCE_SHA'] = current
        cases = [(timing, version, 'warm', 30) for timing in (False, True)
                 for version in ('old', 'new', 'new', 'old')]
        cases.extend((True, version, 'cold-paced', 5) for version in ('old', 'new', 'new', 'old'))
        manifest['plannedCases'] = cases
        for index, (timing, version, cadence, samples) in enumerate(cases):
            name = f'{index:02}-{version}-timing{int(timing)}-{cadence}'
            report, job = output/(name+'.json'), output/(name+'.job.json')
            native_env['DIY_EXPERIMENT_RETAINED_TIMING'] = str(int(timing))
            native_env['DIY_MEASUREMENT_BUILD_SOURCE_SHA'] = current
            native_env['DIY_PRIMITIVE_RETENTION_REPORT'] = str(report)
            command = list(map(str, [sys.executable, 'scripts/measure-primitive-retention-job.py', '--command', job,
                bundle/'runtime/node.exe', 'scripts/benchmark-primitive-retention.mjs', hosts[version],
                bundle/'bin/diy-ime2-node-manager.exe', bundle/'bin/diy-ime2-wasm-worker.exe',
                bundle/'runtime/wasmtime.dll', bundle/'official', 20, samples, 'identity', cadence]))
            case = {'name': name, 'timing': timing, 'version': version, 'cadence': cadence,
                    'samples': samples, 'command': command}
            manifest['cases'].append(case)
            save()
            print('Starting '+name, flush=True)
            start = time.monotonic()
            with (output/(name+'.log')).open('x', encoding='utf-8') as log:
                result = subprocess.run(command, cwd=repo, env=native_env, stdout=log, stderr=log, timeout=330)
            case.update(exitCode=result.returncode, seconds=time.monotonic()-start)
            save()
            assert result.returncode == 0, 'Native trial failed; preserve evidence and stop without retry'
            data, owned = read(report), read(job)
            assert data['completed'] and data['sourceCommit'] == current and data['sampleDeliveryObserve']
            assert data['binaries']['host'] == sha(hosts[version])
            assert data['runtimeBuildSourceCommit'] == current
            assert data['workspaceAfterHostExit'] == [] and owned['allDescendantsExited']
            assert not owned['timedOut'] and owned['missingProcessMemoryReads'] == 0 and owned['driverExitCode'] == 0
            assert data['traceCollections'] and all(t['dropped'] == 0 for t in data['traceCollections'])
            evidence = data['privateReplyEvidence']
            assert evidence['repeatedSnapshotsChecked'] > 0
            if version == 'new':
                assert all(row['repliesWithSamples'] == 0 for method, row in evidence['byMethod'].items() if method != 'status')
            if timing:
                assert evidence['byMethod']['status']['repliesWithSamples'] > 0
                if version == 'old':
                    assert evidence['byMethod']['getRun']['repliesWithSamples'] > 0
            else:
                assert all(row['repliesWithSamples'] == 0 for row in evidence['byMethod'].values())
            case.update(checksPassed=True, replyEvidence=evidence, measurement=data.get('warmResultMs', data.get('pacing')),
                        job={k: v for k, v in owned.items() if k != 'samples'})
            save()
            print(json.dumps({'name': name, 'seconds': case['seconds'], 'checksPassed': True}), flush=True)
        manifest['complete'] = True
    finally:
        save()


if __name__ == '__main__':
    main()
