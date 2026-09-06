// Browser regression checks against mocked API responses; never touches user data.
// Start Vite, then run with PLAYWRIGHT_MODULE pointing to a test-only playwright-core install.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright-core");
const origin = process.env.SEARCH_TEST_ORIGIN || "http://127.0.0.1:4179";
const output = process.env.SEARCH_TEST_OUTPUT || "/tmp/kirara-search-browser";
await mkdir(output, { recursive: true });
const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.SEARCH_TEST_CHROMIUM,
  args: ["--no-sandbox"],
});
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const wait = async (predicate) => {
  for (let i = 0; i < 100; i++) { if (await predicate()) return; await delay(50); }
  throw new Error("Timed out waiting for browser condition");
};
const site = (id, name = `站点 ${id}`) => ({
  id, name, site_type: "nexusphp", base_url: `https://site${id}.example`,
  auth_type: "cookie", auth_configured: true, use_proxy: false,
  created_at: "2026-09-06T00:00:00Z", updated_at: "2026-09-06T00:00:00Z", stats: null,
});
const sites = Array.from({ length: 25 }, (_, i) => site(i + 1));
sites[1].name = sites[0].name;
const task = {
  id: 1, name: "主力刷流站", site_id: 1, enabled: true, last_status: "failed", last_message: "连接超时",
  browser: "lightpanda", sign_in_method: "open_page", browserless: {}, cron_expression: "0 0 0/8 * * *",
  last_run_at: null, downloader_ids: [], tag: "主力", rss_url: "https://feed.example/rss",
  promotion: "free", max_concurrent: 5, skip_hit_and_run: true, delete_mode: "free_end", last_run_info: null,
};
const requests = [];
let creates = 0, updates = 0, bindingWrites = 0;
let clampCount = 45;
let completedAt = Infinity;
const errors = [];
const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
await context.route("**/api/**", async (route) => {
  const request = route.request();
  const url = new URL(request.url());
  const q = url.searchParams.get("q") || "";
  requests.push({ path: url.pathname, q, page: url.searchParams.get("page"), method: request.method() });
  let body = {}, status = 200;
  if (url.pathname.endsWith("/search")) {
    if (q === "slow") await delay(700);
    if (q === "error") { status = 500; body = { error: "测试搜索失败" }; }
    else {
      const size = Number(url.searchParams.get("page_size") || 20);
      const requestedPage = Number(url.searchParams.get("page") || 1);
      let records;
      if (url.pathname.includes("site-catalog")) records = [{ id: "qingwa", name: "QingWa", url: "https://new.qingwa.pro/", aka: ["青蛙"] }];
      else if (url.pathname.includes("sign-in-records")) records = [{ id: 800, task_id: 1, site_id: 1, site_name: "历史青蛙", status: "failed", message: "历史日志跨页命中", started_at: "2026-09-01", finished_at: "2026-09-01" }];
      else if (url.pathname.includes("tasks")) records = [{ ...task, name: Date.now() >= completedAt ? "后台任务已完成" : task.name }];
      else if (q === "clamp") records = Array.from({ length: clampCount }, (_, i) => site(i + 1));
      else if (!q) records = sites;
      else records = [site(1, q === "slow" ? "过期结果" : q === "fast" ? "最新结果" : "主力刷流站")];
      const page = Math.min(requestedPage, Math.max(1, Math.ceil(records.length / size)));
      const parsed_filters = ["health", "type", "site_id"].filter((field) => url.searchParams.has(field)).map((field) => ({ field, value: url.searchParams.get(field) }));
      body = { items: records.slice((page - 1) * size, page * size).map((record) => ({ record, matched_by: ["catalog_alias"] })), total: records.length, page, page_size: size, parsed_filters, semantic_status: q === "clamp" ? "busy" : "not_needed" };
    }
  } else if (url.pathname.endsWith("/run")) {
    completedAt = Date.now() + 1000;
  } else if (url.pathname.endsWith("/search-binding")) {
    if (request.method() === "PUT" && ++bindingWrites === 1) { status = 500; body = { error: "测试关联保存失败" }; }
    else body = { site_id: 99, mode: "auto", catalog_id: null };
  } else if (url.pathname === "/api/sites" && request.method() === "POST") { creates++; body = { id: 99 }; }
  else if (url.pathname === "/api/sites/99" && request.method() === "PUT") { updates++; body = { ok: true }; }
  else if (url.pathname === "/api/sites") body = sites;
  else if (url.pathname === "/api/sign-in-tasks") body = [task];
  else if (url.pathname === "/api/settings") body = { log_level: "info", proxy: null, lightpanda: { region: "euwest" }, browserless: {}, use_proxy_for_lightpanda: true };
  else if (url.pathname === "/api/sites/ptd-backup") body = { site_identifiers: {}, backup_interval_hours: 24 };
  else if (url.pathname === "/api/sites/refresh-all") body = { refreshing: false };
  else if (["/api/downloaders", "/api/sites/catalog", "/api/sign-in-profiles"].includes(url.pathname)) body = [];
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) }).catch(() => {});
});
const page = await context.newPage();
page.on("pageerror", (error) => errors.push(error.message));
try {
  await page.goto(`${origin}/#/sites`);
  await page.locator("#site-search").waitFor();
  await wait(() => page.getByRole("navigation", { name: "站点分页" }).isVisible());
  await page.getByRole("navigation", { name: "站点分页" }).getByRole("button", { name: "下一页" }).click();
  await wait(() => page.getByText("第 2 / 2 页", { exact: false }).isVisible());
  assert(await page.getByText("站点 21", { exact: true }).first().isVisible());
  const input = page.locator("#site-search");
  await input.fill("青蛙");
  await wait(() => page.getByText("主力刷流站", { exact: true }).first().isVisible());

  // A shrinking result set clamps page 3 to page 2; a subsequent growth must retain page 2.
  await input.fill("clamp");
  const pager = page.getByRole("navigation", { name: "站点分页" });
  await wait(() => pager.getByText("第 1 / 3 页", { exact: false }).isVisible());
  await pager.getByRole("button", { name: "下一页" }).click();
  await wait(() => pager.getByText("第 2 / 3 页", { exact: false }).isVisible());
  clampCount = 25;
  await pager.getByRole("button", { name: "下一页" }).click();
  await wait(() => pager.getByText("第 2 / 2 页", { exact: false }).isVisible());
  await wait(() => requests.at(-1).q === "clamp" && requests.at(-1).page === "2");
  clampCount = 45;
  await page.getByRole("button", { name: "重试", exact: true }).click();
  await wait(() => pager.getByText("第 2 / 3 页", { exact: false }).isVisible());
  await input.fill("");
  await page.locator("#site-status-filter").click();
  await page.getByRole("option", { name: "同步失败", exact: true }).click();
  await page.getByRole("button", { name: "清除查询条件" }).click();
  await wait(() => page.locator("#site-status-filter").innerText().then((text) => text.includes("全部状态")));
  await input.fill("青蛙");
  await wait(() => page.getByText("主力刷流站", { exact: true }).first().isVisible());
  assert.equal(await page.getByText("站点 21", { exact: true }).count(), 0, "query hides prior page");
  // Returned custom name intentionally does not contain q: there must be no client filtering.
  await input.fill("slow");
  await wait(() => requests.some((request) => request.q === "slow"));
  await input.fill("fast");
  await wait(() => page.getByText("最新结果", { exact: true }).first().isVisible());
  await delay(800);
  assert.equal(await page.getByText("过期结果", { exact: true }).count(), 0);
  const before = requests.length;
  await input.dispatchEvent("compositionstart");
  await input.fill("输入法尚未完成");
  await delay(300);
  assert.equal(requests.slice(before).filter((request) => request.q === "输入法尚未完成").length, 0);
  await input.fill("青蛙");
  await input.dispatchEvent("compositionend");
  await wait(() => page.getByText("主力刷流站", { exact: true }).first().isVisible());
  await input.fill("error");
  await page.getByRole("button", { name: "重新搜索" }).waitFor();
  assert.equal(await page.getByText("还没有配置 PT 站点", { exact: true }).count(), 0);
  await input.fill("青蛙");
  await wait(() => page.getByText("主力刷流站", { exact: true }).first().isVisible());

  await page.getByRole("button", { name: "添加站点", exact: true }).click();
  await page.locator("#site-name").fill("同名自定义站点");
  await page.locator("#site-base-url").fill("https://custom.example");
  await page.locator("#site-auth-cookie").fill("test-fixture-only");
  await page.locator("#site-search-binding-mode").click();
  await page.getByRole("option", { name: "手动指定", exact: true }).click();
  await page.locator('input[name="site-catalog-choice"]').waitFor();
  await page.locator('input[name="site-catalog-choice"]').check();
  await page.locator('button[form="site-connection-form"][type="submit"]').click();
  await page.getByText("站点已保存，目录关联保存失败：测试关联保存失败", { exact: true }).waitFor();
  await page.locator('button[form="site-connection-form"][type="submit"]').click();
  await wait(() => page.locator("#site-connection-form").count().then((count) => count === 0));
  assert.equal(creates, 1, "binding retry must not create another site");
  assert.equal(updates, 1);
  assert.equal(bindingWrites, 2);

  for (const [route, inputId] of [["sites", "site-search"], ["sign-in", "sign-in-search"], ["brush-tasks", "brush-task-search"]]) {
    await page.goto(`${origin}/#/${route}`);
    await page.locator(`#${inputId}`).fill("青蛙");
    await wait(() => page.getByText("主力刷流站", { exact: true }).first().isVisible());
    for (const [label, width, height] of [["desktop", 1440, 1000], ["mobile", 390, 844]]) {
      await page.setViewportSize({ width, height });
      await page.screenshot({ path: `${output}/${route}-${label}.png`, fullPage: true });
      const dimensions = await page.evaluate(() => ({ scroll: document.documentElement.scrollWidth, width: window.innerWidth }));
      assert(dimensions.scroll <= dimensions.width + 1, `${route}/${label} overflows horizontally`);
    }
  }
  await page.goto(`${origin}/#/sign-in?view=records`);
  await page.locator("#sign-in-search").fill("青蛙");
  await page.getByText("历史日志跨页命中", { exact: true }).waitFor();
  assert(requests.some((request) => request.path === "/api/sign-in-records/search" && request.q === "青蛙"));
  await page.locator("#sign-in-site-filter").click();
  assert(await page.getByRole("option", { name: "站点 1 · #1", exact: true }).isVisible());
  await page.getByRole("option", { name: "站点 1 · #2", exact: true }).click();
  await page.getByRole("button", { name: "清除查询条件" }).click();
  await wait(() => page.locator("#sign-in-site-filter").innerText().then((text) => text.includes("全部站点")));
  assert.equal(await page.locator("#sign-in-search").inputValue(), "");
  await page.goto(`${origin}/#/brush-tasks`);
  await page.getByRole("button", { name: "立即执行一次", exact: true }).click();
  await page.getByText("后台任务已完成", { exact: true }).waitFor({ timeout: 10000 });
  assert(await page.getByRole("button", { name: "刷新", exact: true }).isVisible());
  assert.deepEqual(errors, [], "no browser runtime errors");
  console.log(JSON.stringify({ passed: true, checks: ["server-only matching", "paging and query reset", "server page clamping", "stale responses", "IME", "error state", "clear parsed filters", "duplicate site names", "background completion refresh", "binding partial-save retry", "three list pages", "historical search", "desktop/mobile layout"], creates, updates, screenshots: output }));
} finally {
  await browser.close();
}
