#!/usr/bin/env python3
"""Manage the local frontend (1234) and backend (3000) on Linux."""
import fcntl
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
RUNTIME = ROOT / '.dev'
STATE = RUNTIME / 'services.json'


def identity(pid):
    try:
        # Fields after comm start at field 3; starttime is field 22.
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return fields[19] if fields[0] != 'Z' else None
    except (FileNotFoundError, ProcessLookupError):
        return None


def load():
    return json.loads(STATE.read_text()) if STATE.exists() else {}


def save(state):
    temporary = STATE.with_suffix('.tmp')
    temporary.write_text(json.dumps(state, indent=2))
    temporary.replace(STATE)


def alive(service):
    return bool(service and identity(service['pid']) == service['identity'])


def stop(state):
    for name in ('frontend', 'backend'):
        service = state.get(name)
        if not alive(service):
            state.pop(name, None)
            continue
        pid = service['pid']
        os.killpg(pid, signal.SIGTERM)
        deadline = time.monotonic() + 15
        while alive(service) and time.monotonic() < deadline:
            time.sleep(0.1)
        if alive(service):
            os.killpg(pid, signal.SIGKILL)
        state.pop(name, None)
        print(f'{name} 已停止', flush=True)
    save(state)


def ensure_port_free(port):
    with socket.socket() as sock:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            sock.bind(('0.0.0.0', port))
        except OSError:
            raise RuntimeError(f'端口 {port} 已被其他服务占用，请先检查并停止该服务；不会切换端口或终止未知进程')


def launch(state, name, command):
    with (RUNTIME / f'{name}.log').open('ab') as log:
        process = subprocess.Popen(command, cwd=ROOT, stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
    stamp = identity(process.pid)
    if stamp is None:
        raise RuntimeError(f'{name} 启动失败，请查看 .dev/{name}.log')
    state[name] = {'pid': process.pid, 'identity': stamp}
    save(state)


def request(path, method='GET'):
    req = urllib.request.Request('http://127.0.0.1:1234' + path, method=method,
        headers={'Host': 'arbitrary-host.example:1234', 'Origin': 'https://arbitrary-origin.example',
                 'Access-Control-Request-Method': 'PUT', 'Access-Control-Request-Headers': 'content-type'})
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req, timeout=3) as response:
        if response.status not in (200, 204):
            raise RuntimeError(f'{path} HTTP {response.status}')
        if response.headers.get('Access-Control-Allow-Origin') != '*':
            raise RuntimeError(f'{path} 未允许任意跨域来源')
        body = response.read()
        if path == '/api/settings' and method == 'GET':
            json.loads(body)


def check(state):
    if not all(alive(state.get(name)) for name in ('frontend', 'backend')):
        raise RuntimeError('前端或后端未运行，请查看 .dev/*.log')
    request('/')
    request('/api/settings')
    request('/api/settings', 'OPTIONS')


def start(state):
    if any(alive(state.get(name)) for name in ('frontend', 'backend')):
        check(state)
        print('服务已运行：http://localhost:1234')
        return
    for port in (1234, 3000):
        ensure_port_free(port)
    data_dir = str(Path(os.environ.get('KIRARA_DATA_DIR') or state.get('data_dir') or ROOT / 'data').resolve())
    state.clear()
    state['data_dir'] = data_dir
    save(state)
    if not (ROOT / 'frontend/node_modules').exists():
        subprocess.run(['npm', '--prefix', 'frontend', 'ci'], cwd=ROOT, check=True)
    print('正在构建后端…', flush=True)
    subprocess.run(['cargo', 'build', '--bin', 'kirara'], cwd=ROOT, check=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
    if not target.is_absolute():
        target = ROOT / target
    try:
        launch(state, 'backend', [str(target / 'debug/kirara'), '--data-dir', data_dir,
                                  '--host', '127.0.0.1', '--port', '3000'])
        launch(state, 'frontend', ['npm', '--prefix', 'frontend', 'run', 'dev'])
        deadline = time.monotonic() + 60
        while True:
            try:
                check(state)
                break
            except Exception:
                if time.monotonic() >= deadline or not all(alive(state.get(n)) for n in ('frontend', 'backend')):
                    raise
                time.sleep(0.5)
    except BaseException:
        stop(state)
        raise
    print(f'已启动：http://localhost:1234（监听 0.0.0.0，允许任意来源）\n数据：{data_dir}\n日志：{RUNTIME}')


def main():
    action = sys.argv[1] if len(sys.argv) == 2 else 'help'
    if action not in ('start', 'stop', 'restart', 'status'):
        print('用法：./dev.sh {start|stop|restart|status}\n可选：KIRARA_DATA_DIR=/path/to/data ./dev.sh start')
        return 0 if action in ('help', '--help', '-h') else 1
    RUNTIME.mkdir(mode=0o700, exist_ok=True)
    with (RUNTIME / 'lock').open('w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = load()
        if action in ('stop', 'restart'):
            stop(state)
        if action in ('start', 'restart'):
            start(state)
        if action == 'status':
            for name in ('frontend', 'backend'):
                print(f'{name}: {"运行中" if alive(state.get(name)) else "已停止"}')
            check(state)
            print('页面、API 代理、任意 Host / Origin 检查通过：http://localhost:1234')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f'错误：{error}', file=sys.stderr)
        sys.exit(1)
