import { test, expect } from "@playwright/test";

test("home page renders the speed test shell", async ({ page }) => {
  await page.goto("http://localhost:3000/");

  await expect(page).toHaveTitle("Speed Test");
  await expect(page.locator("h1")).toHaveText("Your network speed");
  await expect(page.locator(".metric")).toHaveCount(5);
});

test("health endpoint reports ok", async ({ request }) => {
  const res = await request.get("http://localhost:3000/api/health");
  expect(res.ok()).toBeTruthy();
  expect((await res.json()).status).toBe("ok");
});
