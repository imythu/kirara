export default async ({ page: servicePage, context: request }) => {
  const isolated = await servicePage.browser().createBrowserContext();
  let page;
  try {
    page = await isolated.newPage();
    await page.setJavaScriptEnabled(false);
    const cdp = await page.createCDPSession();
    const { frameTree } = await cdp.send("Page.getFrameTree");
    const frameId = frameTree.frame.id;
    let first = true;
    let redirects = 0;
    let status = null;
    let failed = false;
    cdp.on("Fetch.requestPaused", async (event) => {
      try {
        if (event.resourceType !== "Document" || event.frameId !== frameId) {
          await cdp.send("Fetch.failRequest", { requestId: event.requestId, errorReason: "Aborted" });
          return;
        }
        if (event.responseErrorReason) {
          failed = true;
          await cdp.send("Fetch.failRequest", { requestId: event.requestId, errorReason: "Failed" });
          return;
        }
        if (event.responseStatusCode) {
          const redirect = [301, 302, 303, 307, 308].includes(event.responseStatusCode) &&
            event.responseHeaders?.some(h => h.name.toLowerCase() === "location");
          if (redirect && request.followRedirects) {
            if (++redirects > 5) {
              failed = true;
              await cdp.send("Fetch.failRequest", { requestId: event.requestId, errorReason: "Aborted" });
            } else {
              await cdp.send("Fetch.continueResponse", { requestId: event.requestId });
            }
            return;
          }
          status = event.responseStatusCode;
          // The original status is recorded, but its document is never rendered.
          // This prevents target scripts, meta refreshes and subresource requests.
          await cdp.send("Fetch.fulfillRequest", { requestId: event.requestId, responseCode: 200, body: "", responseHeaders: [{name:"Content-Type",value:"text/html"}] });
          return;
        }
        if (first) {
          first = false;
          const headers = new Map(Object.entries(event.request.headers).map(([name, value]) => [name.toLowerCase(), { name, value }]));
          headers.delete("content-length");
          for (const header of request.headers) headers.set(header.name.toLowerCase(), header);
          await cdp.send("Fetch.continueRequest", { requestId: event.requestId, method: request.method,
            headers: [...headers.values()], ...(request.hasBody ? { postData: request.body } : {}) });
        } else {
          await cdp.send("Fetch.continueRequest", { requestId: event.requestId });
        }
      } catch {
        failed = true;
        await cdp.send("Fetch.failRequest", { requestId: event.requestId, errorReason: "Failed" }).catch(() => {});
      }
    });
    await cdp.send("Fetch.enable", { patterns: [{ urlPattern: "*", requestStage: "Request" }, { urlPattern: "*", requestStage: "Response" }] });
    await page.goto(request.url, { waitUntil: "domcontentloaded", timeout: Math.max(1, request.timeoutMs - 250) });
    if (failed || !status) throw new Error("Browser request failed");
    return { data: { status_code: status }, type: "application/json" };
  } finally {
    await isolated.close();
  }
};
