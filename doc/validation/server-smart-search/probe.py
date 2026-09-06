"""Linux HTTP regression/performance probe with a fresh synthetic database.

Uses only reserved hostnames and disabled tasks; never reads user configuration.
Waits for the initial empty refresh before inserting the synthetic sites.
Retains the temporary directory for inspection and always stops its own server.
"""
import argparse, concurrent.futures, hashlib, json, math, os, pathlib, platform, signal, socket, sqlite3, statistics, subprocess, tempfile, threading, time, urllib.request, urllib.parse, urllib.error
P = argparse.ArgumentParser()
P.add_argument('--binary', required=True)
P.add_argument('--output', default='/tmp/kirara-search-http-observations.json')
A = P.parse_args()
result = {'binary': A.binary, 'binary_sha256': hashlib.sha256(pathlib.Path(A.binary).read_bytes()).hexdigest(), 'machine': {'platform': platform.platform(), 'cpu_count': os.cpu_count()}, 'fixture': {'sites': 300, 'sign_tasks': 150, 'brush_tasks': 150, 'all_tasks_enabled': False, 'site_hosts': 'reserved .example'}, 'checks': [], 'metrics': {}}
result['machine']['affinity_cpu_count'] = len(os.sched_getaffinity(0))
for filename, key in [('/sys/fs/cgroup/cpu.max', 'cgroup_cpu_max'), ('/sys/fs/cgroup/memory.max', 'cgroup_memory_max')]:
    try:
        result['machine'][key] = pathlib.Path(filename).read_text().strip()
    except OSError:
        pass
result['machine']['cpu_model'] = next((line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), 'unknown')
run_dir = tempfile.mkdtemp(prefix='kirara-search-http-')
result['data_dir'] = run_dir
s = socket.socket()
s.bind(('127.0.0.1', 0))
port = s.getsockname()[1]
s.close()
base = f'http://127.0.0.1:{port}'
log = open(run_dir + '/server.log', 'w')
proc = None
rss = []
done = threading.Event()

def sample():
    while not done.wait(0.02):
        try:
            d = {line.split(':', 1)[0]: line.split(':', 1)[1].strip() for line in pathlib.Path(f'/proc/{proc.pid}/status').read_text().splitlines() if ':' in line}
            rss.append((time.monotonic(), int(d['VmRSS'].split()[0]), int(d['VmHWM'].split()[0])))
        except (OSError, KeyError):
            pass

def call(path, params=None, method='GET', body=None):
    if params:
        path += '?' + urllib.parse.urlencode(params)
    req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), method=method, headers={'Content-Type': 'application/json'})
    t = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=22) as r:
            status = r.status
            payload = r.read()
    except urllib.error.HTTPError as e:
        status = e.code
        payload = e.read()
    try:
        data = json.loads(payload)
    except Exception:
        data = payload.decode(errors='replace')[:200]
    return (status, data, (time.perf_counter() - t) * 1000)

def check(name, condition, details=None):
    result['checks'].append({'name': name, 'passed': bool(condition), 'details': details})
    print(name, 'PASS' if condition else 'FAIL', flush=True)

def ids(data):
    return [item['record']['id'] for item in data.get('items', [])]

def metric(name, path, params, reps=12):
    obs = []
    statuses = []
    totals = []
    sems = []
    start = time.monotonic()
    for _ in range(reps):
        st, d, ms = call(path, params)
        obs.append(ms)
        statuses.append(st)
        totals.append(d.get('total') if isinstance(d, dict) else None)
        sems.append(d.get('semantic_status') if isinstance(d, dict) else None)
    ordered = sorted(obs)
    sampled = [x[1] for x in rss if x[0] >= start]
    result['metrics'][name] = {'samples_ms': obs, 'p50_ms': statistics.median(obs), 'p95_ms': ordered[max(0, math.ceil(0.95 * len(obs)) - 1)], 'http_statuses': statuses, 'totals': totals, 'semantic_statuses': sems, 'peak_rss_kib': max(sampled, default=None)}
    print(name, result['metrics'][name]['p95_ms'], statuses, flush=True)
try:
    env = {key: os.environ[key] for key in ('PATH', 'SYSTEMROOT', 'WINDIR', 'TMPDIR', 'TEMP', 'TMP') if key in os.environ}
    proc = subprocess.Popen([A.binary, '--host', '127.0.0.1', '--port', str(port), '--data-dir', run_dir], stdout=log, stderr=subprocess.STDOUT, env=env)
    for _ in range(150):
        if proc.poll() is not None:
            raise RuntimeError('server exited during startup')
        try:
            if call('/api/features')[0] == 200:
                break
        except OSError:
            pass
        time.sleep(0.1)
    else:
        raise RuntimeError('server startup timed out')
    time.sleep(1.5)
    st, d, _ = call('/api/sites/refresh-all')
    check('initial empty refresh idle', st == 200 and (not d.get('refreshing')), d)
    threading.Thread(target=sample, daemon=True).start()
    cat = json.loads((pathlib.Path(__file__).resolve().parents[3] / 'assets/search/catalog.json').read_text())
    entries = cat['sites']
    pool = [e['id'] for e in entries if e['id'] not in ('qingwa', 'm-team')]
    db = sqlite3.connect(run_dir + '/kirara.db')
    db.execute('PRAGMA foreign_keys=ON')
    now = '2026-09-06T00:00:00Z'
    for i in range(1, 301):
        name = '主力刷流站' if i <= 2 else f'自定义站点{i}'
        db.execute('INSERT INTO sites(id,name,site_type,base_url,auth_config,request_headers,use_proxy,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)', (i, name, 'nexusphp', f'https://site{i}.example', json.dumps({'auth_type': 'cookie', 'cookie': 'CREDENTIAL_NEEDLE_71239'}), '[]', 0, now, now))
        catalog = 'qingwa' if i <= 2 else 'm-team' if i == 3 else pool[(i - 4) % len(pool)]
        db.execute('INSERT INTO site_search_bindings(site_id,mode,catalog_id,catalog_revision) VALUES(?,?,?,?)', (i, 'manual', catalog, 'probe'))
        db.execute('INSERT INTO site_stats(site_id,uid,username,uploaded,downloaded,last_checked_at,last_error) VALUES(?,?,?,?,?,?,?)', (i, str(i), f'user{i}', 100, 50, now, 'synthetic collection failure' if i == 2 else None))
    for i in range(1, 151):
        status = 'failed' if i == 2 else 'success'
        db.execute('INSERT INTO sign_in_tasks(id,name,site_id,cron_expression,lightpanda_token,enabled,last_status,last_message,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)', (i, f'备用签到任务{i}', i, '0 0 0 1 1 *', '', 0, status, 'synthetic event', now, now))
        db.execute('INSERT INTO brush_tasks(id,name,site_id,cron_expression,downloader_ids,tag,rss_url,enabled,last_run_info,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)', (i, f'备用刷流任务{i}', i, '0 0 0 1 1 *', '[]', 'fixture', f'https://rss{i}.example/?passkey=RSS_CREDENTIAL_NEEDLE', 0, json.dumps({'status': status}), now, now))
    for i in range(1, 1001):
        db.execute('INSERT INTO sign_in_records(id,task_id,site_id,site_name,started_at,finished_at,status,message) VALUES(?,1,1,?,?,?,?,?)', (i, '历史名字', now, now, 'failed' if i % 3 == 0 else 'success', 'ancientneedle' if i == 1 else f'event {i}'))
    db.commit()
    time.sleep(0.1)
    result['metrics']['rss_before_search_kib'] = rss[-1][1] if rss else None
    for path in ['/api/sites/search', '/api/sign-in-tasks/search', '/api/brush-tasks/search', '/api/sign-in-records/search', '/api/site-catalog/search']:
        st, d, _ = call(path, {'page_size': 2})
        check('static search route ' + path, st == 200 and 'items' in d, {'status': st, 'total': d.get('total') if isinstance(d, dict) else None})
    st, d, _ = call('/api/sites/search', {'q': '主力刷流站'})
    check('duplicate renamed accounts preserved', st == 200 and ids(d) == [1, 2], ids(d))
    check('site DTO credentials absent', all(('auth_config' not in x['record'] and 'request_headers' not in x['record'] for x in d['items'])))
    st, d, _ = call('/api/sites/search', {'q': 'CREDENTIAL_NEEDLE_71239'})
    check('cookie never searchable', st == 200 and d['total'] == 0)
    st, d, _ = call('/api/brush-tasks/search', {'q': 'RSS_CREDENTIAL_NEEDLE'})
    check('RSS credential never searchable', st == 200 and d['total'] == 0)
    for q, expected in [('QingWa 失败', [2]), ('qingwa 已暂停', [1, 2]), ('zhuli shualiuzhan', [1, 2]), ('site:主力刷流站 result:failed', [2])]:
        st, d, _ = call('/api/sign-in-tasks/search', {'q': q})
        check('alias/pinyin/state ' + q, st == 200 and (ids(d) == expected if expected is not None else bool(d['total'])), {'status': st, 'ids': ids(d), 'filters': d.get('parsed_filters')})
    st, d, _ = call('/api/sites/search', {'q': 'zhulishualiuzhan'})
    check('custom-name full pinyin', st == 200 and ids(d) == [1, 2], ids(d))
    st, d, _ = call('/api/sites/search', {'q': '青蛙 banana'})
    check('hard negative unknown term stays required', st == 200 and d['total'] == 0)
    st, d, _ = call('/api/sites/search', {'page': 99})
    check('page clamps and exact total', st == 200 and d['total'] == 300 and (d['page'] == 15) and (len(d['items']) == 20))
    st, d, _ = call('/api/sign-in-records/search', {'q': 'ancientneedle'})
    check('history older than legacy500 reachable', st == 200 and ids(d) == [1], {'status': st, 'ids': ids(d)})
    st, d, _ = call('/api/sites/1/search-binding', method='PUT', body={'mode': 'none'})
    check('binding none update', st == 200 and d['catalog_id'] is None)
    st, d, _ = call('/api/sites/search', {'q': 'QingWa'})
    check('binding none immediately removes alias', st == 200 and ids(d) == [2], ids(d))
    st, d, _ = call('/api/sites/1/search-binding', method='PUT', body={'mode': 'manual', 'catalog_id': 'qingwa'})
    check('manual binding restored', st == 200 and d['catalog_id'] == 'qingwa')
    st, d, _ = call('/api/sites/1/search-binding', method='PUT', body={'mode': 'manual', 'catalog_id': 'NO_SUCH_CATALOG'})
    check('invalid catalog rejected', st == 400)
    st, d, _ = call('/api/sites/1/search-binding', method='PUT', body={'mode': 'auto'})
    check('reserved hostname not inferred from custom name', st == 200 and d['catalog_id'] is None)
    call('/api/sites/1/search-binding', method='PUT', body={'mode': 'manual', 'catalog_id': 'qingwa'})
    st, d, _ = call('/api/sites/1', method='PUT', body={'name': '主力改名', 'site_type': 'nexusphp', 'base_url': 'https://renamed.example', 'use_proxy': False})
    check('site mutation remains available', st == 200, {'status': st, 'response': d})
    st, d, _ = call('/api/sites/search', {'q': '主力改名'})
    check('rename reflected next search', st == 200 and ids(d) == [1], ids(d))
    st, d, _ = call('/api/sites/1/search-binding')
    check('manual identity survives URL edit', st == 200 and d['catalog_id'] == 'qingwa')
    metric('cold_hybrid', '/api/sites/search', {'q': '适合新手的动漫站'}, 1)
    metric('hot_lexical', '/api/sites/search', {'q': 'QingWa'}, 20)
    metric('hot_hybrid', '/api/sites/search', {'q': '适合新手的动漫站'}, 20)
    metric('hot_task_lexical', '/api/sign-in-tasks/search', {'q': 'qingwa 失败'}, 20)

    def concurrent_request(_):
        return call('/api/sites/search', {'q': '适合新手的动漫站'})
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as ex:
        observations = list(ex.map(concurrent_request, range(12)))
    result['metrics']['four_concurrent_hybrid'] = [{'status': st, 'ms': ms, 'semantic_status': d.get('semantic_status')} for st, d, ms in observations]
    warm_total = result['metrics']['hot_hybrid']['totals'][-1]
    warm_ids = ids(observations[0][1])
    check('warm concurrent hybrid keeps complete semantic results', all(st == 200 and d.get('semantic_status') == 'used' and d.get('total') == warm_total and ids(d) == warm_ids for st, d, _ in observations), [{'status': st, 'semantic_status': d.get('semantic_status'), 'total': d.get('total')} for st, d, _ in observations])
    db.execute('BEGIN')
    for i in range(1001, 100001):
        db.execute('INSERT INTO sign_in_records(id,task_id,site_id,site_name,started_at,finished_at,status,message) VALUES(?,1,1,?,?,?,?,?)', (i, '历史名字', now, now, 'failed' if i % 3 == 0 else 'success', f'synthetic history event {i} ' + 'x' * 128))
    db.commit()
    result['fixture']['large_history_rows'] = 100000
    result['metrics']['rss_before_large_history_kib'] = rss[-1][1]
    metric('history100k_empty_deep', '/api/sign-in-records/search', {'page': 5000}, 1)
    metric('history100k_rare_lexical', '/api/sign-in-records/search', {'q': 'ancientneedle'}, 1)
    metric('history100k_status', '/api/sign-in-records/search', {'q': 'result:failed', 'page': 1600}, 1)
    barrier = threading.Barrier(8)

    def history_request(_):
        barrier.wait()
        return call('/api/sign-in-records/search', {'q': 'nonexistentneedle', 'page': 1})
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as ex:
        pending = [ex.submit(history_request, i) for i in range(8)]
        time.sleep(0.15)
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as ordinary:
            list_during_history = list(ordinary.map(lambda _: call('/api/sites/search', {'q': 'QingWa'}), range(3)))
        result['metrics']['three_list_queries_during_history'] = [{'status': st, 'ms': ms, 'total': d.get('total') if isinstance(d, dict) else None} for st, d, ms in list_during_history]
        check('three ordinary searches remain eligible during history', all((st == 200 for st, _, _ in list_during_history)))
        observations = [future.result() for future in pending]
    result['metrics']['eight_concurrent_history'] = [{'status': st, 'ms': ms, 'total': d.get('total') if isinstance(d, dict) else None} for st, d, ms in observations]
    check('admission limits excess concurrent scans', any((st == 503 for st, _, _ in observations)), [st for st, _, _ in observations])
    check('single history worker completes; seven excess requests rejected', sum((st == 200 for st, _, _ in observations)) == 1 and sum((st == 503 for st, _, _ in observations)) == 7, [st for st, _, _ in observations])
    st, d, _ = call('/api/sites/300', method='DELETE')
    check('site delete available', st == 200)
    st, d, _ = call('/api/sites/search', {'q': 'id:300'})
    check('deleted identity never returned', st == 200 and d['total'] == 0)
    result['metrics']['rss_final_kib'] = rss[-1][1]
    result['metrics']['peak_rss_kib'] = max((x[1] for x in rss))
    result['metrics']['kernel_peak_rss_kib'] = max((x[2] for x in rss))
    result['metrics']['peak_extra_rss_kib'] = result['metrics']['peak_rss_kib'] - result['metrics']['rss_before_search_kib']
    result['metrics']['peak_extra_rss_mib'] = result['metrics']['peak_extra_rss_kib'] / 1024
    result['scheduled_execution_rows'] = db.execute('SELECT count(*) FROM sign_in_records WHERE started_at<>?', (now,)).fetchone()[0]
    result['required_checks_passed'] = all(c['passed'] for c in result['checks'])
    result['required_check_count'] = len(result['checks'])
    result['passed'] = all((x['passed'] for x in result['checks']))
    db.close()
except Exception as e:
    result['fatal_error'] = repr(e)
    result['passed'] = False
    print('ERROR', repr(e), flush=True)
finally:
    done.set()
    if proc is not None:
        proc.terminate()
        try:
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
    log.close()
    result['server_log'] = run_dir + '/server.log'
    pathlib.Path(A.output).write_text(json.dumps(result, ensure_ascii=False, indent=2))
    print('OBSERVATIONS', A.output, flush=True)
if not result.get('passed', False):
    raise SystemExit(1)
