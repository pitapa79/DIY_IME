"""Collect private native startup evidence with the real Windows Host."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


VERIFIED_RUNTIME = "DIY_EXPERIMENT_RETAINED_VERIFIED_RUNTIME_FILE"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load(path):
    return json.loads(path.read_text(encoding="utf-8"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    variant = parser.add_mutually_exclusive_group()
    variant.add_argument("--small-worker", action="store_true",
                         help="substitute the dev debug=0 Worker and verify the baseline inputs")
    variant.add_argument("--verified-runtime", action="store_true",
                         help="compare retained DLL verification using identical baseline inputs")
    variant.add_argument("--verified-runtime-safety", action="store_true",
                         help="check cancellation, revocation and recovery in the verified retained path")
    args = parser.parse_args()
    verified_runtime = args.verified_runtime or args.verified_runtime_safety
    root = Path(__file__).resolve().parent.parent
    target = root / "target"
    baseline_output = target / "retained-native-startup-evidence"
    output = target / "retained-small-worker-evidence" if args.small_worker else baseline_output
    if args.verified_runtime:
        output = target / "retained-verified-runtime-evidence"
    if args.verified_runtime_safety:
        output = target / "retained-verified-runtime-safety-evidence"
    worker_target = target / "retained-small-worker" if args.small_worker else target
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    assert source == os.environ["GITHUB_SHA"]
    official = root / "distribution/official"
    fixture = Path("wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm")
    paths = {
        "node": Path(shutil.which("node")).resolve(strict=True),
        "host": target / "debug/diy-ime2-external-host.exe",
        "manager": target / "debug/diy-ime2-node-manager.exe",
        "worker": worker_target / "debug/diy-ime2-wasm-worker.exe",
        "dll": target / "tools/wasmtime50/wasmtime-v50.0.1-x86_64-windows-c-api/lib/wasmtime.dll",
        "fixtureA": target / fixture,
        "fixtureB": target / "retained-native-startup-alt" / fixture,
        "official": official / "official.wasm",
        "officialManifest": official / "demo.identity.manifest.json",
    }
    files = {name: digest(path) for name, path in paths.items()}
    output.mkdir()
    plan = [1] if args.verified_runtime_safety else [1, 4, 20]
    state = {
        "source": source,
        "files": files,
        "workerBuild": {
            "profile": "dev",
            "debugOverride": 0 if args.small_worker else None,
            "imageBytes": paths["worker"].stat().st_size,
        },
        "plan": plan,
        "results": [],
        "notRun": plan,
    }
    env = {key: value for key, value in os.environ.items() if not key.startswith("DIY_EXPERIMENT_")}
    env.update({
        "DIY_REVIEW_MODULE_A": str(paths["fixtureA"]),
        "DIY_REVIEW_MODULE_B": str(paths["fixtureB"]),
        "DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION": "separate",
        "DIY_EXPERIMENT_RETAINED_TIMING": "1",
        "DIY_EXPERIMENT_RETAINED_NATIVE_STARTUP_TIMING": "1",
        "DIY_EXPERIMENT_PACKAGE_PREPARATION_TIMING": "1",
        "DIY_EXPERIMENT_RETAINED_COMPILED_JIT": "1",
        "DIY_EXPERIMENT_RETAINED_STARTUP_PARALLELISM": "2",
        "DIY_EXPERIMENT_COMPILED_JIT": "1",
        "DIY_PRIMITIVE_MATRIX": "1",
        "DIY_PRIMITIVE_SAMPLE_DELIVERY_CHECK": "1",
        "DIY_PRIMITIVE_PHASE_JOURNAL": "1",
    })
    if verified_runtime:
        env[VERIFIED_RUNTIME] = "1"
    state["flags"] = {key: val for key, val in env.items()
                      if key.startswith(("DIY_EXPERIMENT_", "DIY_PRIMITIVE_"))}
    if args.verified_runtime_safety:
        env["DIY_PRIMITIVE_PYTHON"] = sys.executable

    def save():
        state["evidence"] = {
            path.name: digest(path) for path in output.iterdir() if path.name != "manifest.json"
        }
        (output / "manifest.json").write_text(json.dumps(state, indent=2), encoding="utf-8")

    save()
    try:
        if args.small_worker or verified_runtime:
            baseline_path = baseline_output / "manifest.json"
            baseline = load(baseline_path)
            state["baselineManifestSha256"] = digest(baseline_path)
            assert baseline["source"] == source and baseline["plan"] == [1, 4, 20], "Baseline source or plan differs"
            expected_flags = baseline["flags"].copy()
            assert VERIFIED_RUNTIME not in expected_flags, "Baseline already requested retained runtime proof"
            if verified_runtime:
                expected_flags[VERIFIED_RUNTIME] = "1"
            assert expected_flags == state["flags"], "Baseline runtime flags differ"
        if verified_runtime:
            assert baseline["files"] == files, "Baseline file hashes differ"
            assert baseline["workerBuild"] == state["workerBuild"], "Baseline Worker build differs"
        if args.small_worker:
            assert baseline["workerBuild"]["profile"] == "dev", "Baseline Worker was not built in dev profile"
            assert baseline["workerBuild"]["debugOverride"] is None, "Baseline Worker debug level was overridden"
            assert baseline["files"].keys() == files.keys(), "Baseline file set differs"
            for key in files.keys() - {"worker"}:
                assert baseline["files"][key] == files[key], f"Baseline {key} hash differs"
            assert baseline["files"]["worker"] != files["worker"], "Worker images are identical"
            assert state["workerBuild"]["imageBytes"] < baseline["workerBuild"]["imageBytes"], "Worker image did not shrink"
        for count in plan:
            cadence = "safety" if args.verified_runtime_safety else "cold-paced"
            stem = f"{cadence}-{count:02}"
            report_path = output / (stem + ".json")
            job_path = output / (stem + ".job.json")
            row = {"count": count, "stem": stem, "checksPassed": False}
            state["results"].append(row)
            state["notRun"] = plan[len(state["results"]):]
            env["DIY_PRIMITIVE_RETENTION_REPORT"] = str(report_path)
            command = [
                sys.executable, "scripts/measure-primitive-retention-job.py", "--command",
                str(job_path), str(paths["node"]), "scripts/benchmark-primitive-retention.mjs",
                *(str(paths[key]) for key in ("host", "manager", "worker", "dll")),
                str(official), str(count), "3", "thirdparty", cadence,
            ]
            print(f"Starting {count} nodes", flush=True)
            with (output / (stem + ".log")).open("x", encoding="utf-8") as log:
                row["exitCode"] = subprocess.run(
                    command, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=330
                ).returncode
            assert row["exitCode"] == 0, f"{stem} failed; see artifact"
            report, job = load(report_path), load(job_path)
            assert report["completed"] and report["sourceCommit"] == source
            assert report["runtimeBuildSourceCommit"] == source
            assert report["requestedControls"][VERIFIED_RUNTIME] == env.get(VERIFIED_RUNTIME)
            assert all(report["binaries"][key] == files[key] for key in ("host", "manager", "worker", "dll"))
            assert report["workspaceAfterHostExit"] == []
            assert all(row["dropped"] == 0 for row in report["traceCollections"])
            assert job["allDescendantsExited"] and not job["timedOut"] and job["missingProcessMemoryReads"] == 0
            native = report["retainedNativeStartup"]
            assert not native["truncated"]
            if args.verified_runtime_safety:
                expected = {
                    "controlled-active-host-input-cancel-original-exit-and-recovery",
                    "protection-response-fence-original-process-drain-and-recovery",
                    "revocation-drain-consent-and-recovery",
                    "owned-original-process-fault-no-retroactive-result-change-and-recovery",
                    "close-session-and-fresh-reopen",
                    "plan-session-replacement-and-old-key-fence",
                }
                checks = {item["check"] for item in report["safety"] if item.get("passed")}
                assert expected <= checks
                cancelled = report["controlledCancellation"]
                assert cancelled["passed"] and cancelled["outcome"] == "cancelled"
                assert cancelled["pid"] in {sample["pid"] for sample in native["records"]}
                row.update(checksPassed=True, safetyChecks=sorted(checks), rows=len(native["records"]))
                save()
                continue
            first = report["pacedRounds"][0]
            assert first["intervalMs"] == 20 and first["round"] == 0
            actual_pids = first["status"]["retainedWorker"]["processIds"]
            assert len(set(actual_pids)) == count and all(pid > 0 for pid in actual_pids)
            assert set(actual_pids) <= {sample["pid"] for sample in native["records"]}
            phases = [json.loads(line) for line in (output / (stem + ".log")).read_text(encoding="utf-8").splitlines()
                      if line.startswith('{"diagnostic":"package-preparation-phase",')]
            common = {"package-verify-node", "package-capture-module", "metadata-describe", "metadata-validate",
                      "worker-profile", "worker-assets", "worker-snapshot-directory", "worker-snapshot-image-write",
                      "worker-snapshot-dll-write", "worker-snapshot-locks", "worker-runtime-acl-lease",
                      "worker-launch-and-settle", "worker-cleanup"}
            for role, expected in (("manager", common), ("host", common | {
                "host-prepare", "host-initial-catalog", "host-plan-and-catalog-recheck"
            })):
                started = {(p["pid"], p["id"], p["phase"]) for p in phases
                           if p["role"] == role and p["event"] == "start"}
                completed = {p["phase"] for p in phases if p["role"] == role and p["event"] == "complete"
                             and (p["pid"], p["id"], p["phase"]) in started}
                assert expected <= completed, f"{role} preparation phases missing: {sorted(expected - completed)}"
            row.update(checksPassed=True, firstLatest=first["latestInput"], rows=len(native["records"]))
            print(json.dumps(row), flush=True)
            save()
        state["complete"] = True
    except BaseException as error:
        state["failure"] = {"type": type(error).__name__, "message": str(error)}
        raise
    finally:
        save()


if __name__ == "__main__":
    main()
