// Renders the PNG icons and the social card from the SVGs here, then writes favicon.ico.
// Needs Playwright's Chromium and Python with Pillow: `node render.mjs` in this folder.
import { chromium } from 'playwright';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';

const here = (f) => pathToFileURL(resolve(import.meta.dirname, f)).href;
const browser = await chromium.launch({ chromiumSandbox: false, executablePath: process.env.CHROMIUM || undefined });
const page = await browser.newPage();
// Start from a file: page so the icons below may load sibling files.
await page.goto(here('og.html'));

async function png(svg, size, out) {
	await page.setViewportSize({ width: size, height: size });
	await page.setContent(`<style>html,body{margin:0;background:transparent}</style><img src="${here(svg)}" width="${size}" height="${size}">`);
	await page.waitForFunction(() => document.images[0]?.complete);
	await page.screenshot({ path: resolve(import.meta.dirname, out), omitBackground: true });
}

await png('app-icon.svg', 512, 'icon-512.png');
await png('app-icon.svg', 192, 'icon-192.png');
await png('maskable.svg', 512, 'maskable-512.png');
for (const s of [16, 32, 48]) await png('app-icon-small.svg', s, `favicon-${s}.png`);

await page.setViewportSize({ width: 1200, height: 630 });
await page.goto(here('og.html'));
await page.waitForTimeout(400);
await page.screenshot({ path: resolve(import.meta.dirname, 'og.png') });
await browser.close();

// iOS rounds the corners itself, so the touch icon fills its square with the icon's ground.
execFileSync('python3', ['-I', '-c', `
from PIL import Image
d = ${JSON.stringify(import.meta.dirname)}
icons = [Image.open(f"{d}/favicon-{s}.png").convert("RGBA") for s in (16, 32, 48)]
icons[2].save(f"{d}/favicon.ico", sizes=[(16, 16), (32, 32), (48, 48)], append_images=icons[:2])
im = Image.open(f"{d}/icon-512.png").convert("RGBA")
bg = Image.new("RGBA", im.size, (22, 22, 21, 255)); bg.alpha_composite(im)
bg.convert("RGB").resize((180, 180), Image.LANCZOS).save(f"{d}/apple-touch-icon.png", optimize=True)
`]);
console.log('rendered');
