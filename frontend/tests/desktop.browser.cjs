// PLAYWRIGHT_MODULE=/path/to/playwright DESKTOP_TEST_URL=http://127.0.0.1:5197 node frontend/tests/desktop.browser.cjs
// Prepare with: npm --prefix frontend ci, then npm --prefix frontend run build.
// Serve in another terminal: python3 -m http.server 5197 --bind 127.0.0.1 --directory frontend/dist
// The transport harness is built from source below. Backend commands are mocked; no account data is accessed.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { buildSync } = require('esbuild');
const assert = require('node:assert/strict');
const path = require('node:path');

const url = process.env.DESKTOP_TEST_URL || 'http://127.0.0.1:5197';
const transport = buildSync({
  stdin: {
    contents: 'export * from "./api"; export { desktopFetch, desktopLogs, isDesktop } from "./desktop";',
    resolveDir: path.resolve(__dirname, '../src/lib'),
    loader: 'ts',
  },
  bundle: true,
  format: 'iife',
  globalName: 'DesktopTransport',
  platform: 'browser',
  define: { 'import.meta.env.VITE_APP_VERSION': '"test"' },
  write: false,
}).outputFiles[0].text;

function installDesktopMock() {
  const callbacks = new Map();
  let nextCallback = 1;
  const state = {
    requests: [], routes: {}, pending: {}, opens: [], closes: [], logBehaviors: [],
    emit(index, event, sequence) {
      const attempt = state.opens[index];
      callbacks.get(attempt.channelId)?.({
        index: sequence ?? attempt.sequence++, message: event,
      });
    },
    resolveOpen(index) { state.opens[index].resolve(state.opens[index].id); },
  };
  const defaultSettings = {
    log_level: 'info', proxy: null, lightpanda: {}, browserless: {},
    tag_rule_scan_interval_mins: 7,
  };
  window.isTauri = true;
  window.__desktopTest = state;
  window.__TAURI_INTERNALS__ = {
    transformCallback(callback, once = false) {
      const id = nextCallback++;
      callbacks.set(id, data => {
        if (once) callbacks.delete(id);
        callback(data);
      });
      return id;
    },
    unregisterCallback(id) { callbacks.delete(id); },
    async invoke(command, args) {
      if (command === 'api_request') {
        state.requests.push(args.request);
        const route = state.routes[args.request.path];
        if (route?.reject) throw route.reject;
        if (route?.pending) return new Promise(resolve => { state.pending[args.request.path] = resolve; });
        const data = route?.data ?? (args.request.path === '/api/settings' ? defaultSettings
          : args.request.path === '/api/features' ? { self_use: false } : []);
        return {
          status: route?.status ?? 200,
          statusText: route?.statusText ?? 'OK',
          body: route?.rawBody ?? JSON.stringify(data),
        };
      }
      if (command === 'logs_open') {
        const attempt = { id: `logs-${state.opens.length}`, channelId: args.onEvent.id, sequence: 0 };
        state.opens.push(attempt);
        const behavior = state.logBehaviors.shift();
        if (behavior === 'reject') throw 'log service unavailable';
        if (behavior === 'pending') return new Promise(resolve => { attempt.resolve = resolve; });
        return attempt.id;
      }
      if (command === 'logs_close') {
        state.closes.push(args.id);
        return null;
      }
      throw new Error(`Unexpected desktop command: ${command}`);
    },
  };
}

(async () => {
  const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
  try {
    const page = await browser.newPage();
    page.setDefaultTimeout(6000);
    const errors = [], networkApiRequests = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(installDesktopMock);
    await page.route('**/api/**', route => {
      networkApiRequests.push(route.request().url());
      return route.abort();
    });
    await page.route('**/__desktop_transport_test__', route => route.fulfill({
      contentType: 'text/html', body: '<!doctype html><title>Desktop transport test</title>',
    }));
    await page.goto(`${url}/__desktop_transport_test__`);
    await page.addScriptTag({ content: transport });

    const requests = await page.evaluate(async () => {
      const { api, desktopFetch } = DesktopTransport;
      const state = window.__desktopTest;
      state.routes['/api/read?query=%E4%BA%91%E6%AF%8D'] = { data: { name: '云母' } };
      state.routes['/api/fail'] = { status: 409, statusText: 'Conflict', data: { error: '重复任务' } };
      state.routes['/api/unavailable'] = { status: 503, statusText: 'Unavailable', rawBody: 'temporarily unavailable' };
      state.routes['/api/empty'] = { status: 204 };
      state.routes['/api/native-error'] = { reject: 'desktop backend stopped' };
      const read = await api('/api/read?query=%E4%BA%91%E6%AF%8D');
      await api('/api/write', { method: 'post', body: JSON.stringify({ title: '云母', enabled: false }) });
      const empty = await api('/api/empty');
      const errors = [];
      for (const name of ['/api/fail', '/api/unavailable', '/api/native-error']) {
        try { await api(name); } catch (error) { errors.push({ name: error.name, message: error.message, status: error.status }); }
      }
      const beforeInvalidBody = state.requests.length;
      try { await desktopFetch('/api/form', { body: new FormData() }); }
      catch (error) { errors.push({ name: error.name, message: error.message }); }
      return { read, empty, errors, invalidBodySent: state.requests.length !== beforeInvalidBody, requests: state.requests };
    });
    assert.deepEqual(requests.read, { name: '云母' });
    assert.equal(requests.empty, undefined);
    assert.deepEqual(requests.requests[0], { path: '/api/read?query=%E4%BA%91%E6%AF%8D', method: 'GET', body: null });
    assert.deepEqual(requests.requests[1], { path: '/api/write', method: 'POST', body: '{"title":"云母","enabled":false}' });
    assert.deepEqual(requests.errors.slice(0, 3), [
      { name: 'ApiError', message: '重复任务', status: 409 },
      { name: 'ApiError', message: 'Unavailable', status: 503 },
      { name: 'Error', message: 'desktop backend stopped', status: undefined },
    ]);
    assert.equal(requests.errors[3].name, 'TypeError');
    assert.equal(requests.invalidBodySent, false);

    const aborted = await page.evaluate(async () => {
      const state = window.__desktopTest;
      const before = state.requests.length;
      const preAborted = new AbortController();
      preAborted.abort();
      let preAbortName;
      try { await DesktopTransport.api('/api/never-start', { signal: preAborted.signal }); }
      catch (error) { preAbortName = error.name; }
      const preAbortSent = state.requests.length !== before;
      state.routes['/api/pending'] = { pending: true };
      const controller = new AbortController();
      const result = DesktopTransport.api('/api/pending', { signal: controller.signal })
        .then(() => 'resolved', error => error.name);
      controller.abort();
      const duringAbortName = await result;
      state.pending['/api/pending']({ status: 200, statusText: 'OK', body: '{"late":true}' });
      return { preAbortName, preAbortSent, duringAbortName };
    });
    assert.deepEqual(aborted, { preAbortName: 'AbortError', preAbortSent: false, duringAbortName: 'AbortError' });

    await page.evaluate(() => {
      window.logEvents = [];
      window.subscription = DesktopTransport.subscribeLogs({
        onOpen: () => logEvents.push('open'),
        onLog: data => logEvents.push(data),
        onError: () => logEvents.push('error'),
      });
      const state = window.__desktopTest;
      state.emit(0, { type: 'open' }, 0);
      state.emit(0, { type: 'data', data: 'second' }, 2);
      state.emit(0, { type: 'data', data: 'first' }, 1);
    });
    assert.deepEqual(await page.evaluate(() => logEvents), ['open', 'first', 'second']);
    await page.evaluate(() => __desktopTest.emit(0, { type: 'end' }, 3));
    await page.waitForFunction(() => __desktopTest.opens.length === 2);
    assert.deepEqual(await page.evaluate(() => __desktopTest.closes), ['logs-0']);
    await page.evaluate(() => {
      __desktopTest.emit(0, { type: 'data', data: 'stale' }, 4);
      __desktopTest.emit(1, { type: 'open' });
      __desktopTest.emit(1, { type: 'data', data: 'reconnected' });
      subscription.close();
      __desktopTest.emit(1, { type: 'data', data: 'after-close' });
    });
    assert.deepEqual(await page.evaluate(() => logEvents), ['open', 'first', 'second', 'error', 'open', 'reconnected']);
    assert.deepEqual(await page.evaluate(() => __desktopTest.closes), ['logs-0', 'logs-1']);

    // Resolve the native stream ID after the consumer has already closed it.
    await page.evaluate(() => {
      __desktopTest.logBehaviors.push('pending');
      const pending = DesktopTransport.subscribeLogs({ onOpen() {}, onLog() {}, onError() {} });
      pending.close();
      __desktopTest.resolveOpen(2);
    });
    await page.waitForFunction(() => __desktopTest.closes.includes('logs-2'));

    // A failed open schedules a retry; closing before the timer fires cancels it.
    await page.evaluate(() => {
      __desktopTest.logBehaviors.push('reject');
      window.failedOpenErrors = 0;
      window.failedSubscription = DesktopTransport.subscribeLogs({ onOpen() {}, onLog() {}, onError() { failedOpenErrors++; } });
    });
    await page.waitForFunction(() => failedOpenErrors === 1);
    await page.evaluate(() => failedSubscription.close());
    await page.waitForTimeout(2100);
    assert.equal(await page.evaluate(() => __desktopTest.opens.length), 4);

    // Smoke-test the production UI with native requests, settings writes, and live logs.
    await page.goto(`${url}/#/system-settings`);
    await page.getByRole('heading', { name: '系统设置', exact: true }).first().waitFor();
    await page.getByRole('button', { name: '保存系统设置', exact: true }).click();
    await page.waitForFunction(() => __desktopTest.requests.some(request => request.path === '/api/settings' && request.method === 'PUT'));
    await page.getByRole('button', { name: '实时日志', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: '实时日志', exact: true });
    await page.waitForFunction(() => __desktopTest.opens.length === 1);
    await page.evaluate(() => {
      __desktopTest.emit(0, { type: 'open' });
      __desktopTest.emit(0, { type: 'data', data: JSON.stringify({ encoded_line: encodeURIComponent('2026-09-05 INFO desktop log fixture') }) });
    });
    await dialog.getByText('已连接', { exact: true }).waitFor();
    await dialog.getByText('2026-09-05 INFO desktop log fixture', { exact: true }).waitFor();
    await dialog.getByRole('button', { name: '关闭实时日志', exact: true }).click();
    await page.waitForFunction(() => __desktopTest.closes.includes('logs-0'));
    assert.deepEqual(networkApiRequests, [], 'desktop mode must use native commands for API and logs');

    // Without Tauri, the same API module must keep using browser HTTP and EventSource.
    await page.goto(`${url}/__desktop_transport_test__`);
    await page.evaluate(() => {
      window.isTauri = false;
      window.browserRequests = [];
      window.fetch = async (input, init) => {
        browserRequests.push({ input, method: init.method ?? 'GET' });
        return Response.json({ browser: true });
      };
      window.EventSource = class {
        constructor(input) { this.url = input; window.browserLogSource = this; }
        addEventListener(type, handler) { if (type === 'log') this.onLog = handler; }
        close() { this.closed = true; }
      };
    });
    await page.addScriptTag({ content: transport });
    const web = await page.evaluate(async () => {
      const result = await DesktopTransport.api('/api/browser');
      const events = [];
      const source = DesktopTransport.subscribeLogs({
        onOpen: () => events.push('open'), onLog: data => events.push(data), onError: () => events.push('error'),
      });
      browserLogSource.onopen();
      browserLogSource.onLog({ data: 'browser-log' });
      browserLogSource.onerror();
      source.close();
      return {
        result, events, requests: browserRequests, nativeRequests: __desktopTest.requests,
        logUrl: browserLogSource.url, closed: browserLogSource.closed,
      };
    });
    assert.deepEqual(web, {
      result: { browser: true }, events: ['open', 'browser-log', 'error'],
      requests: [{ input: '/api/browser', method: 'GET' }], nativeRequests: [],
      logUrl: '/api/system/logs/stream', closed: true,
    });
    assert.deepEqual(errors, []);
    console.log('PASS: native JSON requests, HTTP/native errors, 204, cancellation, channel ordering, reconnect, stale messages, close races, settings save, live-log UI and browser HTTP/EventSource fallback.');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
