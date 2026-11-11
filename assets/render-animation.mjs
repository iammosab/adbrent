import puppeteer from "puppeteer-core";
import fs from "node:fs";
const [page_url, out, fpsArg, only] = process.argv.slice(2);
const fps = Number(fpsArg || 30);
const browser = await puppeteer.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true, args: ["--hide-scrollbars"] });
const page = await browser.newPage();
const errors = [];
page.on("pageerror", e => errors.push(String(e)));
page.on("console", m => { if (m.type() === "error") errors.push(m.text()); });
await page.setViewport({ width: 1600, height: 900, deviceScaleFactor: 1 });
await page.goto(page_url + "?t=0", { waitUntil: "networkidle0" });
await page.evaluate(() => document.fonts.ready);
const length = await page.evaluate(() => window.animationLength);
fs.mkdirSync(out, { recursive: true });
const times = only ? only.split(",").map(Number) : Array.from({ length: Math.round(length * fps) }, (_, i) => i / fps);
let n = 0;
for (const t of times) {
  await page.evaluate(t => window.renderAt(t), t);
  await page.screenshot({ path: `${out}/${only ? "t" + t : String(n).padStart(4, "0")}.png` });
  n++;
}
await browser.close();
console.log("frames", n, "errors", errors.length, errors.slice(0, 3).join(" | "));
