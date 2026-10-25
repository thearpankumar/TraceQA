import { defineConfig } from "@playwright/test";

// The same three engines the Python tests use, at the size the extraction read the page at.
// The browsers are the ones `playwright install` put in the shared cache: this package pins the
// same Playwright version as uv.lock (tests/test_codegen.py checks), so the revisions match.
const desktop = { width: 1280, height: 800 };

export default defineConfig({
  testDir: ".",
  testMatch: "shop.spec.ts",
  reporter: "list",
  retries: 0,
  workers: 1,
  projects: [
    { name: "chromium", use: { browserName: "chromium", viewport: desktop } },
    { name: "firefox", use: { browserName: "firefox", viewport: desktop } },
    { name: "webkit", use: { browserName: "webkit", viewport: desktop } },
  ],
});
