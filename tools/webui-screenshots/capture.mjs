// Screenshots every AgentSpace web UI view against the mock API server.
//
// Usage:
//   node capture.mjs [outDir]
//
// Env:
//   BASE_URL   default http://127.0.0.1:8010
//   THEMES     comma separated, default "light,dark"
//   ONLY       comma separated view ids, default all
//   WIDTH/HEIGHT  viewport, default 1440x900
//
// See docs/PLAYWRIGHT.md for environment setup.
import { chromium } from "playwright";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import nodePath from "node:path";
import { fileURLToPath } from "node:url";

const HERE = fileURLToPath(new URL(".", import.meta.url));
const outDir = nodePath.resolve(process.argv[2] ?? nodePath.join(HERE, "out"));
const baseUrl = process.env.BASE_URL ?? "http://127.0.0.1:8010";
const themes = (process.env.THEMES ?? "light,dark").split(",");
const only = process.env.ONLY ? process.env.ONLY.split(",") : null;
const viewport = {
  width: Number(process.env.WIDTH ?? 1440),
  height: Number(process.env.HEIGHT ?? 900),
};

// [view id, steps to click in order from a fresh page load]
//
// A step is an accessible button name, or `{ css }` for elements that have no
// stable accessible name (list rows built from live data, for example).
const views = [
  ["chat", ["Chat"]],
  ["chat-session", ["Chat", { css: ".session-row-button" }]],
  ["agents", ["Agents"]],
  ["workspaces", ["Workspaces"]],
  ["sessions", ["Sessions"]],
  ["kernels", ["Running kernels"]],
  ["memory", ["Memory"]],
  ["gateways", ["Gateways"]],
  ["skills", ["Skills"]],
  ["info", ["System info"]],
  ["config", ["Configuration", "Declarative"]],
  ["config-secrets", ["Configuration", "Secrets"]],
  ["config-kernels", ["Configuration", "Kernels"]],
  ["connections", ["Configuration", "Connections"]],
];

// Fallback for hosts without the Chromium shared libraries installed (see
// bootstrap-sysroot.py). When the sysroot is absent we rely on system packages.
const sysroot = process.env.PW_SYSROOT ?? nodePath.join(HERE, ".sysroot");
const browserEnv = { ...process.env };
const MULTIARCH_BY_ARCH = { x64: "x86_64-linux-gnu", arm64: "aarch64-linux-gnu" };
if (existsSync(sysroot)) {
  // bootstrap-sysroot.py records the triplet it built for; fall back to this
  // host's for a sysroot produced before that marker existed.
  const marker = nodePath.join(sysroot, ".multiarch");
  const multiarch = existsSync(marker)
    ? readFileSync(marker, "utf8").trim()
    : MULTIARCH_BY_ARCH[process.arch];
  if (!multiarch) {
    throw new Error(`unsupported architecture ${process.arch} for the Chromium sysroot`);
  }
  browserEnv.LD_LIBRARY_PATH = [
    nodePath.join(sysroot, "usr/lib", multiarch),
    nodePath.join(sysroot, "lib", multiarch),
    process.env.LD_LIBRARY_PATH,
  ].filter(Boolean).join(":");
  browserEnv.FONTCONFIG_FILE = nodePath.join(sysroot, "etc/fonts/fonts.conf");
  browserEnv.XDG_DATA_DIRS = nodePath.join(sysroot, "usr/share");
}

if (!existsSync(nodePath.join(homedir(), ".fonts")) && !existsSync("/usr/share/fonts/truetype")) {
  console.warn("warning: no fonts found. Chromium aborts with SIGTRAP when Skia cannot");
  console.warn("         resolve a font. See docs/PLAYWRIGHT.md.");
}

mkdirSync(outDir, { recursive: true });

let failures = 0;
for (const theme of themes) {
  for (const [id, path] of views) {
    if (only && !only.includes(id)) continue;
    // One browser per view: a single long-lived browser accumulates enough
    // renderer memory to get OOM-killed part way through the matrix.
    const browser = await chromium.launch({ args: ["--disable-gpu"], env: browserEnv });
    const ctx = await browser.newContext({ viewport });
    const page = await ctx.newPage();
    // A view that throws still paints something, so treat runtime errors as
    // failures rather than letting a broken view screenshot its way to green.
    const pageErrors = [];
    page.on("pageerror", (e) => {
      pageErrors.push(String(e).slice(0, 200));
      console.log(`  [pageerror ${id}]`, String(e).slice(0, 200));
    });
    await page.addInitScript((t) => {
      localStorage.setItem("theme", t);
      localStorage.setItem("sidebar-collapsed", "false");
    }, theme);
    try {
      await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
      await page.waitForTimeout(1500);
      for (const step of path) {
        const target = typeof step === "string"
          ? page.getByRole("button", { name: step, exact: true })
          : page.locator(step.css);
        await target.first().click({ timeout: 5000 });
        await page.waitForTimeout(600);
      }
      await page.waitForTimeout(1400);
      await page.screenshot({ path: nodePath.join(outDir, `${theme}-${id}.png`) });
      if (pageErrors.length > 0) {
        throw new Error(`${pageErrors.length} page error(s): ${pageErrors[0]}`);
      }
      console.log("ok  ", theme, id);
    } catch (err) {
      failures += 1;
      console.log("FAIL", theme, id, String(err).split("\n")[0]);
    }
    await ctx.close().catch(() => {});
    await browser.close().catch(() => {});
  }
}

console.log(`done -> ${outDir}${failures ? ` (${failures} failed)` : ""}`);
process.exitCode = failures ? 1 : 0;
