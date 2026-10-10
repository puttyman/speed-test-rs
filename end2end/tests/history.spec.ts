import { test, expect } from "@playwright/test";
import { BASE, savedRun, uid, watchErrors } from "./helpers";

test("history lists saved runs grouped by label, with rename and delete", async ({ page }) => {
  test.setTimeout(180_000);
  const errors = watchErrors(page);
  const id = uid();
  const a = `Hist A ${id}`;
  const b = `Hist B ${id}`;
  const runA1 = await savedRun(page, a);
  await savedRun(page, a);
  await savedRun(page, b);

  await page.goto(`${BASE}/history`);
  const groupA = page.locator(".run-group", { has: page.locator(".run-group-title", { hasText: a }) });
  const groupB = page.locator(".run-group", { has: page.locator(".run-group-title", { hasText: b }) });
  await expect(groupA.locator(".run-row")).toHaveCount(2, { timeout: 10_000 });
  await expect(groupB.locator(".run-row")).toHaveCount(1);
  // The header shows the label's medians from /api/labels.
  await expect(groupA.locator(".group-medians .m").first()).toContainText(/\d/);

  // Collapse and expand.
  await groupA.locator(".collapse-button").click();
  await expect(groupA.locator(".run-list")).toBeHidden();
  await groupA.locator(".collapse-button").click();
  await expect(groupA.locator(".run-list")).toBeVisible();

  // Rename one of A's runs: it moves to the new label's group.
  const renamed = `Hist C ${id}`;
  await groupA.locator(".run-row").first().locator('button[aria-label="Rename"]').click();
  await page.locator(".rename-input").fill(renamed);
  await page.locator(".rename-input").press("Enter");
  const groupC = page.locator(".run-group", { has: page.locator(".run-group-title", { hasText: renamed }) });
  await expect(groupC.locator(".run-row")).toHaveCount(1);
  await expect(groupA.locator(".run-row")).toHaveCount(1);

  // Delete B's run, with confirmation.
  await groupB.locator('button[aria-label="Delete"]').click();
  await expect(groupB.locator(".confirm")).toContainText("Delete this run?");
  await groupB.locator(".confirm button", { hasText: "Delete" }).click();
  await expect(groupB).toHaveCount(0);
  const left = await (await page.request.get(`${BASE}/api/runs?label=${encodeURIComponent(b)}`)).json();
  expect(left.runs).toHaveLength(0);

  // Label filter and export links follow the filters.
  await page.locator(".filters select").selectOption(a);
  await expect(page.locator(".run-group")).toHaveCount(1);
  await expect(page.locator(".export-links a").first()).toHaveAttribute("href", /label=Hist%20A/);

  // A row links to its run page.
  await page.locator(".run-row .run-main").first().click();
  await expect(page).toHaveURL(/\/runs\//);
  expect(runA1).toBeTruthy();
  expect(errors).toEqual([]);
});

test("compare two labels shows deltas against the first", async ({ page }) => {
  test.setTimeout(180_000);
  const errors = watchErrors(page);
  const id = uid();
  const a = `Cmp A ${id}`;
  const b = `Cmp B ${id}`;
  await savedRun(page, a);
  await savedRun(page, b);

  // Select both labels in history, then Compare.
  await page.goto(`${BASE}/history`);
  for (const l of [a, b]) {
    const group = page.locator(".run-group", { has: page.locator(".run-group-title", { hasText: l }) });
    await group.locator(".label-check input").check();
  }
  await expect(page.locator(".compare-bar")).toContainText("2 labels selected");
  await page.locator(".compare-go").click();
  await expect(page).toHaveURL(/\/compare\?labels=/);

  const cols = page.locator(".cmp-col");
  await expect(cols).toHaveCount(2, { timeout: 10_000 });
  await expect(cols.nth(0).locator(".cmp-name")).toHaveText(a);
  await expect(cols.nth(1).locator(".cmp-name")).toHaveText(b);
  // The first is the reference; the second carries delta badges with arrows.
  await expect(cols.nth(0).locator(".delta.is-ref").first()).toBeVisible();
  await expect(cols.nth(1).locator(".delta").first()).toHaveText(/[+−].*[↑↓]/);
  await expect(page.locator(".best-tag").first()).toBeVisible();
  // Overlaid charts.
  await expect(page.locator(".strip-plot")).toHaveCount(2);
  await expect(page.locator(".strip-row")).toHaveCount(4);
  expect(errors).toEqual([]);
});

test("run page renders a saved run", async ({ page }) => {
  test.setTimeout(120_000);
  const errors = watchErrors(page);
  const label = `Run ${uid()}`;
  const id = await savedRun(page, label);
  await page.goto(`${BASE}/runs/${id}`);
  await expect(page.locator("#run-title")).toHaveText(label);
  await expect(page.locator(".hero-value .value")).toHaveText(/\d/);
  await expect(page.locator(".metric-tile")).toHaveCount(5);
  await expect(page.locator(".detail-card")).toHaveCount(4);
  await expect(page.locator(".server-card")).toContainText("Quick");

  // Unknown runs get a friendly message.
  await page.goto(`${BASE}/runs/00000000-0000-0000-0000-000000000000`);
  await expect(page.getByText("Run not found")).toBeVisible();
  expect(errors.filter((e) => !e.includes("404"))).toEqual([]);
});
