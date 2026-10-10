import { expect, type Browser, type Page } from "@playwright/test";

export const BASE = process.env.BASE_URL ?? "http://localhost:3000";

/** A short random suffix, so parallel tests don't see each other's labels. */
export function uid(): string {
  return Math.random().toString(36).slice(2, 8);
}

/**
 * Requests the browser cancels because the test reloaded or navigated while the previous
 * page was still loading (e.g. the WASM bundle). Firefox and WebKit report these as errors;
 * Chromium does not. They are not application errors.
 */
const NAVIGATION_ABORT = [/The operation was aborted/, /^AbortError/, /NS_BINDING_ABORTED/, /^(TypeError: )?Load failed$/];

export function isNavigationAbort(message: string): boolean {
  return NAVIGATION_ABORT.some((re) => re.test(message.trim()));
}

/** Collects console errors and page errors, except navigation aborts. */
export function watchErrors(page: Page): string[] {
  const errors: string[] = [];
  const push = (m: string) => {
    if (!isNavigationAbort(m)) errors.push(m);
  };
  page.on("pageerror", (e) => push(e.message));
  page.on("console", (m) => m.type() === "error" && push(m.text()));
  return errors;
}

/** Waits until hydration has applied the stored settings (the label is prefilled). */
export async function hydrated(page: Page) {
  await expect(page.locator(".label-text")).not.toBeEmpty({ timeout: 15_000 });
}

/** Opens the test page with stored settings (quick profile by default). */
export async function setup(
  page: Page,
  opts: { label?: string; name?: string; transport?: string; profile?: string } = {},
) {
  await page.goto(`${BASE}/`);
  // Let the first load finish before reloading, so the WASM download isn't aborted.
  await hydrated(page);
  await page.evaluate((o) => {
    localStorage.setItem("speedtest.profile", o.profile ?? "quick");
    localStorage.setItem("speedtest.transport", o.transport ?? "websocket");
    if (o.label) localStorage.setItem("speedtest.last_label", o.label);
    if (o.name) localStorage.setItem("speedtest.device_name", o.name);
  }, opts);
  await page.reload();
  await hydrated(page);
  if (opts.label) await expect(page.locator(".label-text")).toHaveText(opts.label);
}

/**
 * Starts a run. Other tests run in parallel against the same server, so the
 * "others are testing" prompt may appear: answer "Test anyway".
 */
export async function startRun(page: Page) {
  await page.locator(".start-button").click();
  const anyway = page.locator(".busy-prompt button", { hasText: "Test anyway" });
  const stop = page.locator(".start-button", { hasText: "Stop" });
  const again = page.locator(".start-button", { hasText: "Test again" });
  await expect(stop.or(anyway).or(again).first()).toBeVisible({ timeout: 10_000 });
  if (await anyway.isVisible()) await anyway.click();
}

export async function runToCompletion(page: Page) {
  await startRun(page);
  await expect(page.locator(".start-button")).toHaveText("Test again", { timeout: 120_000 });
}

/** Runs a quick test with `label` and returns the saved run's id. */
export async function savedRun(page: Page, label: string): Promise<string> {
  await setup(page, { label });
  await runToCompletion(page);
  await expect(page.locator(".save-notice .saved")).toBeVisible({ timeout: 10_000 });
  const res = await page.request.get(`${BASE}/api/runs?label=${encodeURIComponent(label)}&limit=5`);
  const body = await res.json();
  expect(body.runs.length).toBeGreaterThan(0);
  return body.runs[0].id as string;
}

/** A new browser context ("device") with its own storage. */
export async function newDevice(browser: Browser, name: string, label: string) {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  const errors = watchErrors(page);
  await setup(page, { name, label });
  return { ctx, page, errors };
}
