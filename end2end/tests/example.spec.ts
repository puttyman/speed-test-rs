import { test, expect, type Page } from "@playwright/test";

const BASE = process.env.BASE_URL ?? "http://localhost:3000";

/** Waits until hydration has applied the stored settings (the label is prefilled). */
async function hydrated(page: Page) {
  await expect(page.locator(".label-text")).not.toBeEmpty({ timeout: 15_000 });
}

/** Selects the quick profile (stored per device) and reloads. */
async function useQuickProfile(page: Page, transport = "websocket") {
  await page.goto(`${BASE}/`);
  await page.evaluate((t) => {
    localStorage.setItem("speedtest.profile", "quick");
    localStorage.setItem("speedtest.transport", t);
  }, transport);
  await page.reload();
  await hydrated(page);
}

async function runToCompletion(page: Page) {
  await page.locator(".start-button").click();
  await expect(page.locator(".start-button")).toHaveText("Stop");
  await expect(page.locator(".start-button")).toHaveText("Test again", { timeout: 120_000 });
}

test("home page renders the speed test shell", async ({ page }) => {
  await page.goto(`${BASE}/`);
  await expect(page).toHaveTitle("Speed Test");
  await expect(page.locator("h1")).toHaveText("Your network speed");
  await expect(page.locator(".metric-tile")).toHaveCount(5);
  await expect(page.locator(".timeline-step")).toHaveCount(4);
  await expect(page.locator(".site-nav a")).toHaveText(["Test", "History"]);
});

test("health endpoint reports ok", async ({ request }) => {
  const res = await request.get(`${BASE}/api/health`);
  expect(res.ok()).toBeTruthy();
  expect((await res.json()).status).toBe("ok");
});

for (const transport of ["websocket", "http"]) {
  test(`quick run over ${transport} completes and is saved`, async ({ page, request }) => {
    test.setTimeout(150_000);
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    page.on("console", (m) => m.type() === "error" && errors.push(m.text()));

    await useQuickProfile(page, transport);
    const label = (await page.locator(".label-text").textContent())!.trim();
    await runToCompletion(page);

    // Download, upload, latency and jitter tiles are numeric.
    const values = await page.locator(".metric-tile .value").allTextContents();
    for (const v of values.slice(0, 4)) expect(v).toMatch(/^\d/);

    const info = await (await request.get(`${BASE}/api/info`)).json();
    if (info.history) {
      await expect(page.locator(".save-notice .saved")).toBeVisible({ timeout: 10_000 });
      const runs = await (await request.get(`${BASE}/api/runs?limit=1`)).json();
      expect(runs.runs[0].label).toBe(label);
      expect(runs.runs[0].transport).toBe(transport);

      // Inline rename.
      await page.locator(".save-notice .link-button").click();
      await page.locator("#rename-input").fill(`Renamed ${transport}`);
      await page.locator("#rename-input").press("Enter");
      await expect(page.locator(".save-notice .saved")).toContainText(`Renamed ${transport}`);
      const run = await (await request.get(`${BASE}/api/runs/${runs.runs[0].id}`)).json();
      expect(run.label).toBe(`Renamed ${transport}`);
    }
    expect(errors).toEqual([]);
  });
}

test("units and theme persist across reloads", async ({ page }) => {
  await page.goto(`${BASE}/`);
  await hydrated(page);
  await page.locator('.header-units [role="radio"]', { hasText: "MB/s" }).click();
  await page.locator(".theme-toggle").click(); // System -> Light
  await page.locator(".theme-toggle").click(); // Light -> Dark
  await page.reload();
  await hydrated(page);
  await expect(page.locator('.header-units [aria-checked="true"]')).toHaveText("MB/s");
  await expect(page.locator(".metric-tile .unit").first()).toHaveText("MB/s");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
});

test("settings drawer opens and closes with the keyboard", async ({ page }) => {
  await page.goto(`${BASE}/`);
  await hydrated(page);
  await page.locator("#settings-button").click();
  await expect(page.locator("#settings-drawer")).toHaveClass(/is-open/);
  await page.keyboard.press("Escape");
  await expect(page.locator("#settings-drawer")).not.toHaveClass(/is-open/);
  await expect(page.locator("#settings-button")).toBeFocused();
});

test("360 px wide layout has no horizontal scroll", async ({ browser }) => {
  const ctx = await browser.newContext({ viewport: { width: 360, height: 780 } });
  const page = await ctx.newPage();
  await page.goto(`${BASE}/`);
  await hydrated(page);
  const [scroll, client] = await page.evaluate(() => [
    document.documentElement.scrollWidth,
    document.documentElement.clientWidth,
  ]);
  expect(scroll).toBeLessThanOrEqual(client);
  await ctx.close();
});

test("reduced motion replaces the particles with a static gradient", async ({ browser }) => {
  const ctx = await browser.newContext({ reducedMotion: "reduce" });
  const page = await ctx.newPage();
  await page.goto(`${BASE}/`);
  await hydrated(page);
  await expect(page.locator(".packet-flow")).toHaveClass(/is-static/);
  await expect(page.locator(".flow-canvas")).toBeHidden();
  await ctx.close();
});

test("history link is not a 404", async ({ page }) => {
  await page.goto(`${BASE}/`);
  await page.locator(".site-nav a", { hasText: "History" }).click();
  await expect(page.getByText("History is coming soon")).toBeVisible();
});
