// Runs a generated page object against the demo shop. tests/test_codegen.py sets three variables:
//
//   TRACEQA_SHOP_URL     the shop's address
//   TRACEQA_PAGE_OBJECT  the generated <name>.page.ts
//   TRACEQA_EXPECTED     JSON: { className, counts: { <browser>: { <property>: <elements> } } },
//                        taken from the components.json each browser wrote
//
// Every property must find exactly as many elements as the extraction recorded in that browser.
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

import { expect, test } from "@playwright/test";

function need(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set; tests/test_codegen.py sets it`);
  return value;
}

const shop = need("TRACEQA_SHOP_URL");
const pageObject = need("TRACEQA_PAGE_OBJECT");
const expected = JSON.parse(readFileSync(need("TRACEQA_EXPECTED"), "utf8")) as {
  className: string;
  counts: Record<string, Record<string, number>>;
};

test("every property finds what the extraction found", async ({ page }, testInfo) => {
  const counts = expected.counts[testInfo.project.name];
  expect(counts, `no expected counts for ${testInfo.project.name}`).toBeTruthy();
  expect(Object.keys(counts).length).toBeGreaterThan(20);

  const module = await import(pathToFileURL(pageObject).href);
  const shopPage = new module[expected.className](page);
  await page.goto(shop);

  // Load what the shop loads while a person scrolls (the reviews), as the extraction did.
  let height = -1;
  for (let round = 0; round < 12; round++) {
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    await page.waitForTimeout(300);
    const now = await page.evaluate(() => document.body.scrollHeight);
    if (now === height) break;
    height = now;
  }
  await page.evaluate(() => window.scrollTo(0, 0));

  for (const [property, count] of Object.entries(counts)) {
    // Each property is a getter, so reading it builds the locator for the current page.
    await expect.soft(shopPage[property] as import("@playwright/test").Locator, property).toHaveCount(count);
  }
});
