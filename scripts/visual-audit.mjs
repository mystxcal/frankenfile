import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { chromium } = require("../test-output/visual-tools/node_modules/playwright-core");

const [baseUrl, code, dropId, outputDir] = process.argv.slice(2);
if (!baseUrl || !code || !dropId || !outputDir) {
  throw new Error("usage: visual-audit.mjs BASE_URL CODE DROP_ID OUTPUT_DIR");
}

const browser = await chromium.launch({
  executablePath: "/usr/bin/google-chrome",
  headless: true,
  args: ["--no-sandbox", "--disable-dev-shm-usage"],
});

const results = [];
for (const profile of [
  { name: "desktop", viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 },
  { name: "mobile", viewport: { width: 390, height: 844 }, deviceScaleFactor: 1 },
]) {
  const context = await browser.newContext({
    viewport: profile.viewport,
    deviceScaleFactor: profile.deviceScaleFactor,
    colorScheme: "dark",
    reducedMotion: profile.name === "mobile" ? "reduce" : "no-preference",
  });
  const page = await context.newPage();
  const consoleErrors = [];
  const failedRequests = [];
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("requestfailed", (request) => {
    failedRequests.push(`${request.method()} ${request.url()} ${request.failure()?.errorText ?? "failed"}`);
  });

  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.screenshot({ path: `${outputDir}/${profile.name}-landing.png`, fullPage: true });
  const landingMetrics = await page.evaluate(() => ({
    width: innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    height: innerHeight,
    scrollHeight: document.documentElement.scrollHeight,
    title: document.title,
    heading: document.querySelector("h1")?.textContent?.trim(),
    activeElement: document.activeElement?.id,
  }));

  await page.locator("#signal").fill(code);
  const [redeemResponse] = await Promise.all([
    page.waitForResponse((response) => response.url().endsWith("/frankenfile/redeem")),
    page.locator("button[type=submit]").click(),
  ]);
  await page.waitForLoadState("networkidle");
  if (!page.url().endsWith(`/d/${dropId}`)) {
    await page.screenshot({ path: `${outputDir}/${profile.name}-redeem-failure.png`, fullPage: true });
    throw new Error(JSON.stringify({
      profile: profile.name,
      responseStatus: redeemResponse.status(),
      requestBody: redeemResponse.request().postData(),
      requestHeaders: redeemResponse.request().headers(),
      finalUrl: page.url(),
      pageText: (await page.locator("body").innerText()).slice(0, 500),
    }, null, 2));
  }
  await page.screenshot({ path: `${outputDir}/${profile.name}-drop.png`, fullPage: true });
  const dropMetrics = await page.evaluate(() => ({
    width: innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    height: innerHeight,
    scrollHeight: document.documentElement.scrollHeight,
    title: document.title,
    heading: document.querySelector("h1")?.textContent?.trim(),
    entries: document.querySelectorAll(".entry").length,
    visibleActions: [...document.querySelectorAll(".entry-action")].filter((node) => {
      const style = getComputedStyle(node);
      return style.visibility !== "hidden" && style.display !== "none";
    }).length,
  }));

  results.push({
    profile: profile.name,
    landing: landingMetrics,
    drop: dropMetrics,
    consoleErrors,
    failedRequests,
  });
  await context.close();
}

await browser.close();
console.log(JSON.stringify(results, null, 2));
