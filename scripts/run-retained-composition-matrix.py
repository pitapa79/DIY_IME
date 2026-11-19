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


def profile_pool_evidence(report, idle_recovery=False):
    if idle_recovery:
        proof = report['retainedLifetime']
        statuses = [proof['after']]
        recovery, = proof['recoveryAttempts']
        recovered = {(row['nodeId'], row['processId']) for row in proof['sameCall']['rows']
                     if row['status'] == 'paired' and row['key'] == recovery['key']}
    else:
        statuses = [report.get('lifetimeReady'), *report['statuses'],
                    *(round.get('status') for round in report.get('pacedRounds', []))]
    consumed, missed, invoked = set(), set(), set()
    for status in filter(None, statuses):
        for owner in status['retainedWorker'].get('callSamples', []):
            by_attempt = {}
            for phase in owner['calls']['phases']:
                if phase['attempt'] is not None:
                    by_attempt.setdefault(phase['attempt'], {})[phase['phase']] = phase
            for attempt, phases in by_attempt.items():
                key = (status['sessionEpoch'], *owner['nodeIds'], attempt)
                if 'profile-pool-miss' in phases:
                    missed.add(key)
                if 'profile-pool-consumed' in phases:
                    consumed.add(key)
                    resumed = phases.get('native-resume-thread-finished')
                    finished = phases.get('invocation-call-finished')
                    if resumed and finished and resumed['processId'] == finished['processId'] and resumed['processId']:
                        if idle_recovery and (finished['runId'] != recovery['key']['runId']
                                             or (owner['nodeIds'][0], finished['processId']) not in recovered):
                            continue
                        if int(phases['profile-pool-consumed']['atNs']) <= int(resumed['atNs']) <= int(finished['atNs']):
                            invoked.add(key)
    assert not consumed & missed, 'conflicting profile origins for one Worker attempt'
    return {'consumed': sorted(consumed), 'fallback': sorted(missed), 'invoked': sorted(invoked)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('preflight', type=Path)
    parser.add_argument('--candidate', action='store_true',
                        help='enable the two snapshot experiments on retained cases only')
    parser.add_argument('--lifetime', action='store_true',
                        help='exercise the bounded sustained and idle-recovery candidate')
    parser.add_argument('--lifetime-timing', action='store_true',
                        help='capture detailed Native calls and launch phases in the lifetime case')
    parser.add_argument('--idle-overlap', action='store_true',
                        help='enable bounded overlapping replacement in the timed lifetime case')
    parser.add_argument('--ordered-recovery', action='store_true',
                        help='prioritize queued idle replacement starts in dependency order')
    parser.add_argument('--initial-startup', choices=('eager', 'deferred', 'scoped', 'deferred-safety', 'scoped-safety'),
                        help='compare initial startup policies on timed cold input fixtures')
    parser.add_argument('--profile-pool', choices=('fresh', 'prepared'),
                        help='compare fresh and unused prepared AppContainer profiles')
    parser.add_argument('--overlap-action', choices=('cancel', 'protect'),
                        help='interrupt a witnessed idle replacement after the measured recovery')
    args = parser.parse_args()
    if args.lifetime and not args.candidate:
        parser.error('--lifetime requires --candidate')
    if args.lifetime_timing and not args.lifetime:
        parser.error('--lifetime-timing requires --lifetime')
    if args.idle_overlap and not args.lifetime_timing:
        parser.error('--idle-overlap requires --lifetime-timing')
    if args.overlap_action and not args.idle_overlap:
        parser.error('--overlap-action requires --idle-overlap')
    if args.ordered_recovery and not args.idle_overlap:
        parser.error('--ordered-recovery requires --idle-overlap')
    if args.initial_startup and (not args.candidate or args.lifetime):
        parser.error('--initial-startup requires --candidate and cannot use --lifetime')
    if args.profile_pool and (not args.candidate or args.initial_startup):
        parser.error('--profile-pool requires --candidate and excludes --initial-startup')
    if args.profile_pool and args.lifetime and (not args.ordered_recovery or args.overlap_action):
        parser.error('idle profile comparisons require --ordered-recovery without --overlap-action')
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
    module_hashes = {str(path.relative_to(repo)): sha(path) for path in modules}
    if args.candidate:
        assert len(set(module_hashes.values())) == len(modules), 'Wasm fixtures are not distinct'
    candidate_controls = ('DIY_EXPERIMENT_RETAINED_VERIFIED_RUNTIME_FILE',
                          'DIY_EXPERIMENT_CHUNKED_SNAPSHOT_WRITES')
    output.mkdir(parents=True)
    planned = output/'planned-cases.json'
    subprocess.run([str(bundle/'runtime/node.exe'), 'scripts/retained-composition-preflight.mjs',
                    str(bundle/'official'), str(planned)], cwd=repo, check=True)
    cases = read(planned)
    assert len(cases) == 25
    assert {c['layout'] for c in cases} == {'serial', 'parallel', 'join', 'unused'}
    assert {c['count'] for c in cases if c['mode'] == 'separate' and c['layout'] == 'serial'} == {1, 2, 4, 8, 16, 20}
    issue, profile = (158, 'retained-snapshot-candidate') if args.candidate else (135, 'reference')
    if args.profile_pool and not args.lifetime:
        case, = (c for c in cases if c['mode'] == 'separate' and c['timing']
                 and c['cadence'] == 'cold-paced' and c['layout'] == 'serial' and c['count'] == 20)
        cases = [case, dict(case, cadence='paced'), dict(case, cadence='safety', samples=1)]
        planned.write_text(json.dumps(cases, indent=2), encoding='utf-8')
        issue, profile = 178, f'retained-profile-{args.profile_pool}'
    if args.initial_startup in ('deferred-safety', 'scoped-safety'):
        case, = (c for c in cases if c['mode'] == 'separate' and c['timing']
                 and c['cadence'] == 'cold-paced' and c['layout'] == 'serial' and c['count'] == 20)
        cases = [dict(case, cadence='safety', samples=1)]
        planned.write_text(json.dumps(cases, indent=2), encoding='utf-8')
        issue, profile = 176, f'initial-startup-{args.initial_startup}'
    elif args.initial_startup:
        cases = [dict(c, cadence='cold-paced') for c in cases
                 if c['mode'] == 'separate' and c['timing']
                 and c['cadence'] in ('cold-paced', 'paced')]
        assert len(cases) == 8
        planned.write_text(json.dumps(cases, indent=2), encoding='utf-8')
        issue, profile = 176, f'initial-startup-{args.initial_startup}'
    if args.lifetime:
        case, = (c for c in cases if c['mode']=='separate' and c['layout']=='serial'
                 and c['count']==20 and c['cadence']=='warm' and not c['timing'])
        cases = [dict(case, name='retained-lifetime-20', cadence='lifetime', samples=25,
                      timing=args.lifetime_timing)]
        planned.write_text(json.dumps(cases, indent=2), encoding='utf-8')
        issue, profile = (172, 'retained-lifetime-timing') if args.lifetime_timing else (166, 'retained-lifetime')
        if args.idle_overlap:
            issue, profile = 174, 'retained-lifetime-overlap'
        if args.profile_pool:
            issue, profile = 178, f'retained-idle-profile-{args.profile_pool}'
    manifest = {'issue': issue,
                'parentGoal': 109, 'sourceCommit': source,
                'profile': profile,
                'scope': 'Synthetic normalHost topology/input coverage; guest spans include SDK ABI. No semantic/framework target or product adoption claim.',
                'bundleFiles': {path: hashes[path] for path in files},
                'moduleFiles': module_hashes,
                'preflightExeSha256': sha(preflight),
                'harnessFilesSha256': {name: sha(repo/name) for name in [
                    'scripts/run-retained-composition-matrix.py', 'scripts/retained-composition-cases.mjs',
                    'scripts/retained-composition-preflight.mjs', 'scripts/retained-composition-graphs.mjs',
                    'scripts/analyze-retained-graph-calls.mjs', 'scripts/benchmark-primitive-retention.mjs',
                    'scripts/observed-native-ring-history.mjs',
                    'scripts/host-input-scenarios.mjs', 'scripts/host-warm-input-observation.mjs', 'scripts/analyze-retained-call-timing.mjs',
                    'scripts/private-sample-delivery.mjs', 'scripts/measure-primitive-retention-job.py',
                    'editor/src/executionTrace.ts']}, 'plannedCases': cases,
                'cases': [], 'allCasesPhysicallyReviewed': False, 'complete': False, 'notRun': []}
    def save():
        manifest['evidenceFiles'] = {path.name: sha(path) for path in output.iterdir()
                                   if path.is_file() and path.name != 'manifest.json'}
        manifest['notRun'] = [{'name': c['name'], 'reason': 'not yet executed or stopped after earlier failure'}
                              for c in cases[len(manifest['cases']):]]
        (output/'manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
    base = {key: value for key, value in os.environ.items() if not key.startswith('DIY_EXPERIMENT_')}
    base.pop('GITHUB_SHA', None)
    base.pop('DIY_PRIMITIVE_IDLE_OVERLAP_ACTION', None)
    base.pop('DIY_PRIMITIVE_INITIAL_STARTUP_COMPARISON', None)
    if args.initial_startup:
        base['DIY_PRIMITIVE_INITIAL_STARTUP_COMPARISON'] = '1'
    if args.overlap_action:
        base['DIY_PRIMITIVE_IDLE_OVERLAP_ACTION'] = args.overlap_action
        manifest['overlapAction'] = args.overlap_action
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
                if args.candidate:
                    env.update(dict.fromkeys(candidate_controls, '1'))
                if args.profile_pool == 'prepared':
                    env['DIY_EXPERIMENT_RETAINED_PREPARED_PROFILE'] = '1'
                if args.idle_overlap:
                    env['DIY_EXPERIMENT_RETAINED_IDLE_OVERLAP'] = '1'
                if args.ordered_recovery:
                    env['DIY_EXPERIMENT_RETAINED_IDLE_RECOVERY_ORDER'] = '1'
                if args.initial_startup and args.initial_startup != 'eager':
                    env['DIY_EXPERIMENT_RETAINED_DEFER_INITIAL_STARTUP'] = '1'
                if args.initial_startup in ('scoped', 'scoped-safety'):
                    env['DIY_EXPERIMENT_RETAINED_SCOPED_INITIAL_STARTUP'] = '1'
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
            if result.returncode != 0 and (args.profile_pool or args.initial_startup in ('eager', 'deferred', 'scoped')):
                owned = read(job)
                failed_report = read(report)
                assert failed_report['sourceCommit'] == source
                case.update(checksPassed=False, failure=failed_report.get('failure', 'Native case failed'),
                            missingProcessMemoryReads=owned['missingProcessMemoryReads'],
                            workspaceCleanupVerified=failed_report.get('workspaceAfterHostExit') == [])
                save()
                # Each case launches its own fixture. Proceed only once this Job
                # has settled, and preserve the original failure for aggregation.
                assert owned['allDescendantsExited'] and not owned['timedOut']
                assert case['workspaceCleanupVerified'], 'Failed case workspace did not settle'
                continue
            assert result.returncode == 0, 'Failed native case preserved; no retry'
            data, owned = read(report), read(job)
            assert data['completed'] and data['sourceCommit'] == source and data['runtimeBuildSourceCommit'] == source and data['workspaceAfterHostExit'] == []
            assert data['binaries']['host'] == hashes[files[0]]
            assert all(t['dropped'] == 0 for t in data['traceCollections'])
            assert owned['driverExitCode'] == 0 and owned['allDescendantsExited'] and not owned['timedOut']
            if args.candidate:
                expected = {key: env.get(key) for key in data['requestedControls']}
                assert all(key in expected for key in candidate_controls)
                assert all(key in expected for key in case['runtimeFlags'])
                assert data['requestedControls'] == expected
                assert data['binaries'] == dict(zip(('host', 'manager', 'worker', 'dll'),
                                                     (hashes[path] for path in files[:4])))
            if args.profile_pool:
                profiles = profile_pool_evidence(data, idle_recovery=args.lifetime)
                case['preparedProfileEvidence'] = profiles
                save()
                if args.profile_pool == 'prepared':
                    assert profiles['invoked'], 'no actual call used a prepared profile'
                else:
                    assert not any(profiles.values()), 'fresh control touched the prepared profile pool'
                if spec['cadence'] == 'safety':
                    assert data['controlledCancellation']['passed']
                    assert data['workerFaultRecovery']['oldResultFence']
            if args.initial_startup:
                proof = data['initialStartupComparison']
                assert proof['deferred'] == (args.initial_startup != 'eager')
                scoped = args.initial_startup in ('scoped', 'scoped-safety')
                assert proof['scoped'] == scoped
                if scoped and spec['cadence'] == 'safety':
                    assert {guard['action'] for guard in proof['guards'] if guard['passed']} == {'cancel', 'protect'}
                if spec['layout'] == 'unused':
                    assert proof['additionalTarget']['passed']
                    assert proof['additionalTarget']['node'] in data['unexecutedIds']
                if spec['cadence'] == 'safety':
                    assert data['controlledCancellation']['passed']
                    assert all(check.get('passed', True) for check in data['safety'])
            if args.lifetime:
                proof = data['retainedLifetime']
                assert proof['passed'] and proof['idleElapsedMs'] >= 61000
                assert data['sustainedWindowMs'] >= 60000 and data['warmInputDenominators']['correct'] == 26
                assert len(proof['recoveryAttempts']) == 1 and proof['recoveryAttempts'][0]['outcome'] == 'correct'
                case['lifetime'] = {key: proof[key] for key in ('idleElapsedMs', 'recoveryMs', 'previousInput')}
                case['lifetime']['sustainedWindowMs'] = data['sustainedWindowMs']
                if args.lifetime_timing:
                    recovery = proof['recoveryAttempts'][0]['key']
                    rows = proof['sameCall']['rows']
                    assert len(rows) == spec['count'] == proof['sameCall']['paired']
                    owners = {tuple(owner['context'][2]): owner for owner in data['lifecycle']['owners']
                              if owner['context'][:2] == [recovery['sessionEpoch'], recovery['planVersion']]}
                    phases = []
                    for row in rows:
                        assert row['key'] == recovery
                        owner = owners[(row['nodeId'],)]
                        attempt, = (a for a in owner['attempts'] if row['processId'] in a['processIds'])
                        invocation, = (i for i in owner['invocations'] if i['runId'] == recovery['runId'])
                        phases.append({'nodeId': row['nodeId'], 'processId': row['processId'],
                                       'attempt': attempt, 'invocation': invocation})
                    case['lifetime']['recoveryPhases'] = phases
                if args.overlap_action:
                    guard = proof['overlapGuard']
                    assert guard['action'] == args.overlap_action and guard['passed']
                    assert guard['witness']['pid'] > 0 and guard['witness']['runState'] in ('accepted', 'running')
                    if args.overlap_action == 'cancel':
                        assert guard['terminal']['result']['reason'] == 'cancelled'
                        assert guard['exit']['phase'] == 'owner-joined'
                    else:
                        assert guard['terminal']['ok'] is False
                        assert guard['exit']['phase'] == 'protected'
                        assert guard['exit']['workerProcesses'] == 0
                    case['lifetime']['overlapGuard'] = guard['witness']
            missing = owned['missingProcessMemoryReads']
            case.update(checksPassed=missing == 0, resourcePolicy=data['resourcePolicy'],
                        executedIds=data['executedIds'], unexecutedIds=data['unexecutedIds'],
                        warm=data.get('warmResultMs'), pacing=data.get('pacing'),
                        missingProcessMemoryReads=missing)
            if missing:
                # Actual workspace and Job exit have already been verified.
                # Continue independent cases, and fail the diagnostic at the end.
                case['failure'] = 'Windows Job process-memory observation missing'
            save()
        manifest['allCasesPhysicallyReviewed'] = all(
            case.get('workspaceCleanupVerified', True) for case in manifest['cases'])
        assert all(case['checksPassed'] for case in manifest['cases']), 'Native failures or Job memory gaps preserved'
        manifest['complete'] = True
    except BaseException as error:
        manifest['failure'] = {'type': type(error).__name__, 'message': str(error)}
        raise
    finally:
        save()


if __name__ == '__main__':
    main()
