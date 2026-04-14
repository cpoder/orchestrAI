// Records a short demo of the dashboard: pick a plan -> expand phase ->
// open a task's terminal. Saves as webm; convert to gif with ffmpeg.
import { chromium } from "playwright";

const BASE = "http://localhost:3100";
const OUT = new URL(".", import.meta.url).pathname;

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({
  viewport: { width: 1200, height: 720 },
  deviceScaleFactor: 1,
  colorScheme: "dark",
  recordVideo: { dir: OUT, size: { width: 1200, height: 720 } },
});
const page = await context.newPage();

async function pause(ms) {
  await page.waitForTimeout(ms);
}

try {
  await page.goto(BASE);
  await pause(1200);

  // Click a plan with decent content
  const plan = page
    .locator("aside button")
    .filter({ hasText: /Portable Agent Supervisor|Rust Server Rewrite/ })
    .first();
  await plan.click();
  await pause(1500);

  // Expand a phase by clicking its header
  const phaseHeader = page
    .locator("button")
    .filter({ hasText: /Phase \d+:/ })
    .nth(1);
  if (await phaseHeader.count()) {
    await phaseHeader.click();
    await pause(1500);
  }

  // Scroll a bit to show more of the board
  await page.mouse.wheel(0, 300);
  await pause(800);

  // Pick an agent to showcase the terminal
  await page.goto(`${BASE}/`);
  await pause(500);
  await page.locator("nav button").filter({ hasText: "Agents" }).click();
  await pause(1200);

  // Click the newest agent row
  const agentRow = page
    .locator("main button")
    .filter({ hasText: /Task/ })
    .first();
  if (await agentRow.count()) {
    await agentRow.click();
    await pause(2500);
  }
} finally {
  const videoPath = await page.video()?.path();
  await context.close();
  await browser.close();
  console.log("video:", videoPath);
}
