import { test, expect } from "@playwright/test";
import { BASE, newDevice, uid } from "./helpers";

// Several browser contexts on one server: presence, the Devices panel and a
// synchronized group test (SPEC §5.11, §7.11).
test("devices see each other and test together", async ({ browser }) => {
  test.setTimeout(180_000);
  const id = uid();
  const names = [`Mac ${id}`, `Phone ${id}`, `Tablet ${id}`];
  const devices = [];
  for (const [i, n] of names.entries()) {
    devices.push(await newDevice(browser, n, `Room ${i} ${id}`));
  }
  const [a, b, c] = devices;

  // Every device lists the others within about a second.
  for (const d of devices) {
    for (const n of names) {
      await expect(d.page.locator(".device-card", { hasText: n })).toBeVisible({ timeout: 1_500 });
    }
    await expect(d.page.locator(".device-card.is-you")).toContainText(
      names[devices.indexOf(d)],
    );
  }

  // The host creates a group; the others join from the invite toast.
  await a.page.locator(".devices button", { hasText: "Test together" }).click();
  await expect(a.page.locator(".group-bar")).toBeVisible();
  for (const d of [b, c]) {
    const toast = d.page.locator(".invite-toast", { hasText: names[0] });
    await expect(toast).toBeVisible({ timeout: 5_000 });
    await toast.locator(".toast-action", { hasText: "Join" }).click();
    await expect(d.page.locator(".group-bar")).toContainText("Waiting for");
  }
  await expect(a.page.locator(".member-chip")).toHaveCount(3);
  const start = a.page.locator(".group-start");
  await expect(start).toBeEnabled();
  await start.click();

  // Everyone counts down, runs with one lane per member, and gets the result.
  for (const d of devices) {
    await expect(d.page.locator(".countdown-digit")).toBeVisible({ timeout: 5_000 });
  }
  for (const d of devices) {
    await expect(d.page.locator(".lane")).toHaveCount(3, { timeout: 10_000 });
  }
  for (const d of devices) {
    const result = d.page.locator(".group-result");
    await expect(result).toBeVisible({ timeout: 90_000 });
    // Aggregate download and upload.
    await expect(result.locator(".group-totals .metric-value .value").nth(0)).toHaveText(/\d/);
    await expect(result.locator(".group-totals .metric-value .value").nth(1)).toHaveText(/\d/);
    // Each device's share.
    await expect(result.locator(".members-table tbody tr")).toHaveCount(3);
    await expect(result.locator(".share-pct").first()).toHaveText(/\d+%/);
    // Fairness verdict.
    await expect(result.locator(".fairness-verdict").first()).toHaveText(/(Even|Uneven) split/);
    // The stacked charts, with one band per device.
    await expect(result.locator(".stacked-area").first().locator(".stack-band")).toHaveCount(3);
  }

  // The group appears in history as one expandable row.
  await a.page.locator(".hero-actions button", { hasText: "Done" }).click();
  await a.page.goto(`${BASE}/history`);
  const row = a.page.locator(".group-row", { hasText: names[0] }).first();
  await expect(row).toBeVisible({ timeout: 10_000 });
  await row.locator(".group-toggle").click();
  await expect(row.locator(".group-result")).toBeVisible({ timeout: 10_000 });

  for (const d of devices) {
    expect(d.errors).toEqual([]);
    await d.ctx.close();
  }
});

test("hide me shows a device as hidden to others", async ({ browser }) => {
  const id = uid();
  const a = await newDevice(browser, `Seen ${id}`, `Here ${id}`);
  const b = await newDevice(browser, `Hidden ${id}`, `There ${id}`);
  await expect(a.page.locator(".device-card", { hasText: `Hidden ${id}` })).toBeVisible({ timeout: 2_000 });
  await b.page.locator("#settings-button").click();
  await b.page.locator(".setting-switch").click();
  await b.page.keyboard.press("Escape");
  await expect(a.page.locator(".device-card", { hasText: `Hidden ${id}` })).toHaveCount(0, { timeout: 3_000 });
  expect(a.errors).toEqual([]);
  expect(b.errors).toEqual([]);
  await a.ctx.close();
  await b.ctx.close();
});
