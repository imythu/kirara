#!/usr/bin/env python3
"""Linux-only, offline synthetic 1,000-site / 10,000-task HTTP scale measurement.

Run only while builds and other performance probes are stopped. The server is
restricted to two available CPUs. It starts with an empty isolated database;
all fixture jobs remain disabled, and every configured hostname is reserved.
Correctness assertions and measured performance targets are reported separately.
"""
import argparse
import concurrent.futures
import hashlib
import json
import math
import os
import pathlib
import platform
import shutil
import socket
import sqlite3
import statistics
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[3]
NOW = "2026-09-06T00:00:00Z"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--catalog", default=str(ROOT / "assets/search/catalog.json"))
    parser.add_argument("--samples", type=int, default=12)
    parser.add_argument("--build-description", default="not supplied")
    args = parser.parse_args()
    if not 4 <= args.samples <= 30:
        parser.error("samples must be 4..30")
    affinity = sorted(os.sched_getaffinity(0))[:2]
    if len(affinity) != 2:
        parser.error("two available CPUs are required")
    catalog_bytes = pathlib.Path(args.catalog).read_bytes()
    catalog = json.loads(catalog_bytes)["sites"]
    other_ids = [entry["id"] for entry in catalog if entry["id"] != "qingwa"]
    report = {
        "schema_version": 1,
        "fixture": {
            "sites": 1000, "sign_tasks": 5000, "brush_tasks": 5000,
            "unbound_sites": 142, "tasks_per_site_per_scope": 5,
            "duplicate_custom_name_site_ids": [1, 2], "enabled_tasks": 0,
            "hosts": "reserved .example only", "history_rows": 0,
        },
        "machine": {
            "platform": platform.platform(), "host_cpu_count": os.cpu_count(),
            "server_cpu_affinity": affinity,
            "cpu_model": next((line.split(":", 1)[1].strip()
                               for line in pathlib.Path("/proc/cpuinfo").read_text().splitlines()
                               if line.startswith("model name")), "unknown"),
        },
        "catalog_sha256": hashlib.sha256(catalog_bytes).hexdigest(),
        "build_environment": {
            "description": args.build_description,
            "installed_rustc": subprocess.run(["rustc", "--version"], check=True, capture_output=True, text=True).stdout.strip(),
        },
        "correctness": [], "metrics": {},
        "measurement_notes": [
            "HTTP timings include loopback transport and complete JSON response decoding.",
            "Warm p95 uses nearest-rank percentile of the recorded samples.",
            "One shared model has only one process-cold query; later scopes reuse it.",
            "Performance targets are reported, not used to suppress correctness results.",
        ],
    }
    for filename, key in [("/sys/fs/cgroup/cpu.max", "cgroup_cpu_max"),
                          ("/sys/fs/cgroup/memory.max", "cgroup_memory_max")]:
        try:
            report["machine"][key] = pathlib.Path(filename).read_text().strip()
        except OSError:
            pass
    process = None
    rss = []
    stop_sampling = threading.Event()
    sampler = None

    def assert_observation(name, passed, details=None):
        report["correctness"].append({"name": name, "passed": bool(passed), "details": details})
        print(f"{name}: {'PASS' if passed else 'FAIL'}", flush=True)

    try:
        with tempfile.TemporaryDirectory(prefix="kirara-scale-probe-") as temporary:
            directory = pathlib.Path(temporary)
            executable = directory / "server"
            shutil.copyfile(args.binary, executable)
            executable.chmod(0o700)
            report["binary"] = {"source": str(pathlib.Path(args.binary).resolve()),
                                "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                                "size_bytes": executable.stat().st_size}
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1", 0))
                port = reservation.getsockname()[1]
            base = f"http://127.0.0.1:{port}"

            overall_deadline = time.monotonic() + 240

            def request(path, query=None):
                if time.monotonic() >= overall_deadline:
                    raise RuntimeError("240-second probe budget exhausted; partial measurements retained")
                if query:
                    path += "?" + urllib.parse.urlencode(query)
                started = time.perf_counter()
                try:
                    with urllib.request.urlopen(base + path, timeout=20) as response:
                        status, raw = response.status, response.read()
                except urllib.error.HTTPError as error:
                    status, raw = error.code, error.read()
                payload = json.loads(raw)
                return {"status": status, "ms": (time.perf_counter() - started) * 1000,
                        "total": payload.get("total"),
                        "semantic_status": payload.get("semantic_status"),
                        "ids": [item["record"]["id"] for item in payload.get("items", [])]}

            def sample_rss():
                while not stop_sampling.wait(0.02):
                    try:
                        values = dict(line.split(":", 1)
                                      for line in pathlib.Path(f"/proc/{process.pid}/status").read_text().splitlines()
                                      if ":" in line)
                        rss.append((time.monotonic(), int(values["VmRSS"].split()[0]),
                                    int(values["VmHWM"].split()[0])))
                    except (OSError, KeyError):
                        pass

            def summarize(observations, started):
                times = sorted(item["ms"] for item in observations)
                sampled = [item[1] for item in rss if item[0] >= started]
                return {"observations": observations, "p50_ms": statistics.median(times),
                        "p95_ms": times[math.ceil(0.95 * len(times)) - 1],
                        "peak_rss_kib": max(sampled, default=None)}

            def measure(name, path, query, count):
                started = time.monotonic()
                observations = [request(path, {"q": query, "page_size": 20}) for _ in range(count)]
                report["metrics"][name] = summarize(observations, started)
                print(f"{name}: p95={report['metrics'][name]['p95_ms']:.2f}ms "
                      f"statuses={[item['status'] for item in observations]}", flush=True)
                return observations

            def shutdown():
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=8)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                stop_sampling.set()
                if sampler:
                    sampler.join(timeout=1)

            with (directory / "server.log").open("w") as log:
                environment = {key: os.environ[key] for key in
                               ("PATH", "SYSTEMROOT", "WINDIR", "TMPDIR", "TEMP", "TMP")
                               if key in os.environ}
                process = subprocess.Popen(
                    [str(executable), "--host", "127.0.0.1", "--port", str(port),
                     "--data-dir", str(directory / "data")],
                    stdout=log, stderr=subprocess.STDOUT, env=environment,
                    preexec_fn=lambda: os.sched_setaffinity(0, affinity),
                )
                try:
                    for _ in range(150):
                        if process.poll() is not None:
                            raise RuntimeError("server exited during startup")
                        try:
                            if request("/api/features")["status"] == 200:
                                break
                        except OSError:
                            pass
                        time.sleep(0.1)
                    else:
                        raise RuntimeError("server startup timed out")
                    # SiteStatsRefresher performs two immediate empty ticks on startup.
                    time.sleep(1.5)
                    with urllib.request.urlopen(base + "/api/sites/refresh-all") as response:
                        idle = not json.load(response)["refreshing"]
                    assert_observation("initial empty site refresh idle", idle)
                    if not idle:
                        raise RuntimeError("refresher must be idle before inserting fixture")
                    sampler = threading.Thread(target=sample_rss, daemon=True)
                    sampler.start()
                    with sqlite3.connect(directory / "data/kirara.db") as db:
                        db.execute("PRAGMA foreign_keys=ON")
                        for site_id in range(1, 1001):
                            name = "主力刷流站" if site_id <= 2 else f"自定义备用站{site_id}"
                            db.execute("INSERT INTO sites(id,name,site_type,base_url,auth_config,request_headers,use_proxy,created_at,updated_at) VALUES(?,?,?,?,?,'[]',0,?,?)",
                                       (site_id, name, "nexusphp", f"https://account{site_id}.example",
                                        '{"auth_type":"cookie","cookie":"synthetic-only"}', NOW, NOW))
                            catalog_id = "qingwa" if site_id <= 2 else other_ids[(site_id - 3) % len(other_ids)]
                            mode = "none" if site_id % 7 == 0 else "manual"
                            db.execute("INSERT INTO site_search_bindings(site_id,mode,catalog_id,catalog_revision) VALUES(?,?,?,'scale-probe')",
                                       (site_id, mode, None if mode == "none" else catalog_id))
                        for task_id in range(1, 5001):
                            site_id = (task_id - 1) % 1000 + 1
                            status = "failed" if task_id % 3 == 0 else "success"
                            db.execute("INSERT INTO sign_in_tasks(id,name,site_id,cron_expression,lightpanda_token,enabled,last_status,last_message,created_at,updated_at) VALUES(?,?,?,'0 0 0 1 1 *','',0,?,'synthetic message',?,?)",
                                       (task_id, f"签到任务{task_id}", site_id, status, NOW, NOW))
                            db.execute("INSERT INTO brush_tasks(id,name,site_id,cron_expression,downloader_ids,tag,rss_url,enabled,last_run_info,created_at,updated_at) VALUES(?,?,?,'0 0 0 1 1 *','[]','synthetic',?,0,?,?,?)",
                                       (task_id, f"刷流任务{task_id}", site_id, f"https://rss{task_id}.example",
                                        json.dumps({"status": status}), NOW, NOW))
                        db.commit()
                        time.sleep(0.1)
                        report["rss_before_search_kib"] = rss[-1][1]
                        scopes = [("sites", "/api/sites/search", 1000, "主力刷流站"),
                                  ("sign", "/api/sign-in-tasks/search", 5000, "签到任务4321"),
                                  ("brush", "/api/brush-tasks/search", 5000, "刷流任务4321")]
                        for scope, path, total, name in scopes:
                            first = measure(scope + "_first_empty", path, "", 1)[0]
                            assert_observation(scope + " exact unfiltered total", first["status"] == 200 and first["total"] == total, first)
                            named = measure(scope + "_first_name", path, name, 1)[0]
                            expected = [1, 2] if scope == "sites" else [4321]
                            assert_observation(scope + " exact custom name", named["status"] == 200 and named["ids"] == expected, named)
                            alias = request(path, {"q": "QingWa"})
                            assert_observation(scope + " alias/multiple accounts", alias["status"] == 200 and alias["total"] == (2 if scope == "sites" else 10), alias)
                            negative = request(path, {"q": "青蛙 banana"})
                            assert_observation(scope + " hard-negative AND", negative["status"] == 200 and negative["total"] == 0, negative)
                        cold = measure("process_cold_hybrid_sites", "/api/sites/search", "适合新手的动漫站", 1)[0]
                        assert_observation("cold hybrid uses model", cold["status"] == 200 and cold["semantic_status"] == "used", cold)
                        for scope, path, _, name in scopes:
                            for kind, query in [("empty", ""), ("name", name), ("alias", "QingWa"),
                                                ("hybrid", "适合新手的动漫站")]:
                                observations = measure(scope + "_warm_" + kind, path, query, args.samples)
                                target = 200 if kind == "hybrid" else 50
                                report["metrics"][scope + "_warm_" + kind]["target_p95_ms"] = target
                                report["metrics"][scope + "_warm_" + kind]["meets_latency_target"] = report["metrics"][scope + "_warm_" + kind]["p95_ms"] <= target
                                baseline = observations[0]
                                stable = all(item["status"] == 200 and item["total"] == baseline["total"] and item["ids"] == baseline["ids"] for item in observations)
                                if kind == "hybrid":
                                    stable = stable and all(item["semantic_status"] == "used" for item in observations)
                                if kind == "alias":
                                    expected_ids = [1, 2] if scope == "sites" else [site + offset for offset in range(0, 5000, 1000) for site in (1, 2)]
                                    stable = stable and baseline["ids"] == expected_ids and baseline["total"] == len(expected_ids)
                                assert_observation(scope + " warm " + kind + " stable response", stable,
                                                   {"totals": sorted({item["total"] for item in observations}, key=str), "semantic_statuses": sorted({item["semantic_status"] for item in observations}, key=str)})
                            for kind, query in [("alias", "QingWa"), ("hybrid", "适合新手的动漫站")]:
                                started = time.monotonic()
                                with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
                                    observations = list(pool.map(lambda _: request(path, {"q": query}), range(12)))
                                metric = summarize(observations, started)
                                metric["target_p95_ms"] = 200 if kind == "hybrid" else 50
                                metric["meets_latency_target"] = metric["p95_ms"] <= metric["target_p95_ms"]
                                report["metrics"][scope + "_four_concurrent_" + kind] = metric
                                baseline = report["metrics"][scope + "_warm_" + kind]["observations"][0]
                                correct = all(item["status"] == 200 and item["total"] == baseline["total"] and item["ids"] == baseline["ids"] for item in observations)
                                if kind == "hybrid":
                                    correct = correct and all(item["semantic_status"] == "used" for item in observations)
                                assert_observation(scope + " four concurrent " + kind + " preserves full results", correct,
                                                   {"totals": [item["total"] for item in observations], "semantic_statuses": [item["semantic_status"] for item in observations]})
                                print(f"{scope}_four_concurrent_{kind}: p95={metric['p95_ms']:.2f}ms", flush=True)
                        assert_observation("no scheduled execution records", db.execute("SELECT count(*) FROM sign_in_records").fetchone()[0] == 0)
                        assert_observation("all fixture jobs disabled", db.execute("SELECT (SELECT count(*) FROM sign_in_tasks WHERE enabled<>0)+(SELECT count(*) FROM brush_tasks WHERE enabled<>0)").fetchone()[0] == 0)
                        report["peak_rss_kib"] = max(item[1] for item in rss)
                        report["kernel_peak_rss_kib"] = max(item[2] for item in rss)
                        report["peak_extra_rss_mib"] = (report["peak_rss_kib"] - report["rss_before_search_kib"]) / 1024
                        report["meets_extra_memory_target_64_mib"] = report["peak_extra_rss_mib"] <= 64
                finally:
                    shutdown()
            report["server_log_tail"] = (directory / "server.log").read_text()[-4000:]
        report["temporary_database_removed"] = True
    except Exception as error:
        report["fatal_error"] = repr(error)
        if process is not None and process.poll() is None:
            process.kill()
            process.wait()
        stop_sampling.set()
    report["correctness_passed"] = "fatal_error" not in report and all(item["passed"] for item in report["correctness"])
    destination = pathlib.Path(args.output)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(f"observations: {destination}", flush=True)


if __name__ == "__main__":
    main()
