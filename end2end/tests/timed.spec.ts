import { test, expect } from "@playwright/test";
import { BASE, hydrated, newDevice, runToCompletion, uid, watchErrors } from "./helpers";

// Fixed-duration ("Timed") tests (SPEC §5.3, §7.8). CI keeps every speed test under 10 s,
// so the runs use the test-only `?timed_ms=` override instead of the slider's 10 s minimum.

test("the test length slider snaps, is keyboard accessible and persists", async ({ page }) => {
  await page.goto(`${BASE}/`);
  await hydrated(page);
  await page.locator("#settings-button").click();
  const length = page.locator('[role="radiogroup"][aria-label="Test length"]');
  await length.locator('[role="radio"]', { hasText: "Timed" }).click();
  const slider = page.locator("#timed-length");
  await expect(slider).toBeVisible();
  await expect(slider).toHaveAttribute("aria-valuetext", "30 s");
  await expect(page.locator(".length-summary")).toHaveText("Download 30 s, then upload 30 s");
  // The profile choice is replaced by the duration while Timed is selected.
  await expect(page.locator('[role="radiogroup"][aria-label="Profile"]')).toHaveCount(0);
  await slider.focus();
  await page.keyboard.press("ArrowRight");
  await expect(slider).toHaveAttribute("aria-valuetext", "45 s");
  await page.keyboard.press("ArrowRight");
  await expect(slider).toHaveAttribute("aria-valuetext", "1 min");
  await page.keyboard.press("End");
  await expect(slider).toHaveAttribute("aria-valuetext", "5 min");
  await expect(page.locator(".length-summary")).toHaveText("Download 5 min, then upload 5 min");
  await page.keyboard.press("Home");
  await expect(slider).toHaveAttribute("aria-valuetext", "10 s");
  await page.keyboard.press("ArrowRight");
  await expect(slider).toHaveAttribute("aria-valuetext", "15 s");
  await page.keyboard.press("Escape");

  await page.reload();
  await hydrated(page);
  expect(await page.evaluate(() => localStorage.getItem("speedtest.test_length"))).toBe("timed");
  expect(await page.evaluate(() => localStorage.getItem("speedtest.timed_ms"))).toBe("15000");
  await page.locator("#settings-button").click();
  await expect(page.locator("#timed-length")).toHaveAttribute("aria-valuetext", "15 s");
  // Back to Automatic: the profile choice returns.
  await page
    .locator('[role="radiogroup"][aria-label="Test length"] [role="radio"]', { hasText: "Automatic" })
    .click();
  await expect(page.locator('[role="radiogroup"][aria-label="Profile"]')).toBeVisible();
  await expect(page.locator("#timed-length")).toHaveCount(0);
});

test("a timed run counts down and reports peak and average", async ({ page, request }) => {
  test.setTimeout(90_000);
  const errors = watchErrors(page);
  const label = `Timed ${uid()}`;
  await page.goto(`${BASE}/`);
  await hydrated(page);
  await page.evaluate((l) => {
    localStorage.setItem("speedtest.test_length", "timed");
    localStorage.setItem("speedtest.transport", "websocket");
    localStorage.setItem("speedtest.last_label", l);
  }, label);
  // 2.5 s each way (test-only override of the 10 s minimum).
  await page.goto(`${BASE}/?timed_ms=2500`);
  await hydrated(page);
  await expect(page.locator(".label-text")).toHaveText(label);

  const started = Date.now();
  const caption = page.locator(".hero-caption");
  const done = runToCompletion(page);
  await expect(caption).toContainText(/(Download|Upload) · \d+ s left/, { timeout: 15_000 });
  await expect(page.locator(".countdown-ring")).toBeVisible();
  await done;
  expect(Date.now() - started).toBeLessThan(20_000);

  // Tiles: peak and average.
  const down = page.locator('.metric-tile[aria-label="Download"] .metric-detail');
  const up = page.locator('.metric-tile[aria-label="Upload"] .metric-detail');
  await expect(down).toHaveText(/^Peak [\d.,]+ · Avg [\d.,]+ (Mbps|Gbps|kbps|MB\/s|GB\/s|kB\/s)$/);
  await expect(up).toHaveText(/^Peak [\d.,]+ · Avg /);
  // Every phase is done.
  await expect(page.locator(".timeline-step.is-done")).toHaveCount(4);

  // The saved run carries the new summary fields and a throughput series.
  await expect(page.locator(".save-notice .saved")).toBeVisible({ timeout: 10_000 });
  const list = await (await request.get(`${BASE}/api/runs?label=${encodeURIComponent(label)}&limit=5`)).json();
  expect(list.runs.length).toBe(1);
  const run = await (await request.get(`${BASE}/api/runs/${list.runs[0].id}`)).json();
  expect(run.profile).toBe("timed");
  for (const k of ["download_peak_bps", "download_avg_bps", "upload_peak_bps", "upload_avg_bps"]) {
    expect(run.summary[k], k).toBeGreaterThan(0);
  }
  expect(run.summary.download_peak_bps).toBeGreaterThanOrEqual(run.summary.download_avg_bps * 0.9);
  expect(run.points.download_series.length).toBeGreaterThan(2);
  expect(run.points.upload_series.length).toBeGreaterThan(2);
  const timed = run.phases.filter((p: { duration_ms?: number }) => p.duration_ms === 2500);
  expect(timed.length).toBe(2);

  // The run page shows the same: peak/average and the throughput over time.
  await page.goto(`${BASE}/runs/${run.id}`);
  await expect(page.locator('.metric-tile[aria-label="Download"] .metric-detail')).toHaveText(/^Peak /);
  await expect(page.locator(".live-card .chart-svg .series-download")).toHaveCount(1);
  await expect(page.locator(".run-page .kv")).toContainText("Timed · 2.5 s each way");
  expect(errors).toEqual([]);
});

test("the organizer picks the group test length", async ({ browser }) => {
  test.setTimeout(120_000);
  const id = uid();
  const host = await newDevice(browser, `Host ${id}`, `Lab ${id}`);
  const guest = await newDevice(browser, `Guest ${id}`, `Den ${id}`);
  // The host prefers timed tests; `?timed_ms` keeps CI under 10 s.
  await host.page.evaluate(() => localStorage.setItem("speedtest.test_length", "timed"));
  await host.page.goto(`${BASE}/?timed_ms=4000`);
  await hydrated(host.page);
  await expect(host.page.locator(".device-card", { hasText: `Guest ${id}` })).toBeVisible({ timeout: 5_000 });

  await host.page.locator(".devices button", { hasText: "Test together" }).click();
  const bar = host.page.locator(".group-bar");
  await expect(bar).toBeVisible();
  await expect(bar.locator("#group-length")).toHaveAttribute("aria-valuetext", "4 s");

  // The invite says how long.
  const toast = guest.page.locator(".invite-toast", { hasText: `Host ${id}` });
  await expect(toast).toContainText("Test together · 4 s each way", { timeout: 5_000 });
  await toast.locator(".toast-action", { hasText: "Join" }).click();
  await expect(guest.page.locator(".group-bar")).toContainText("4 s each way");
  await expect(host.page.locator(".member-chip")).toHaveCount(2);
  await host.page.locator(".group-start").click();

  for (const d of [host, guest]) {
    await expect(d.page.locator(".hero-caption")).toContainText(/\d+ s left/, { timeout: 10_000 });
  }
  for (const d of [host, guest]) {
    const result = d.page.locator(".group-result");
    await expect(result).toBeVisible({ timeout: 40_000 });
    // 1 s latency + 4 s down + 0.5 s pause + 4 s up.
    await expect(result.locator(".group-result-head")).toContainText(/over (9|10) s/);
    await expect(result.locator(".members-table tbody tr")).toHaveCount(2);
    await expect(result.locator(".share-peak").first()).toHaveText(/^Peak \d/);
  }
  for (const d of [host, guest]) {
    expect(d.errors).toEqual([]);
    await d.ctx.close();
  }
});
