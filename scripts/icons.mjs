// Renders the PNG app icons in assets/icons/ from the logo's shapes.
// Run after changing assets/logo.svg; see scripts/screenshots.mjs for setup:
//   CHROME=/path/to/chrome node scripts/icons.mjs assets/icons
import { writeFileSync } from "node:fs";
import { chromium } from "playwright-core";
const out = process.argv[2];
const glyph = (stroke, bubble) => `
  <path d="M12 51V12l40 10M47 21v30M8 51h48" fill="none" stroke="${stroke}" stroke-width="4.5" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M22 28h16a4.5 4.5 0 0 1 4.5 4.5v6a4.5 4.5 0 0 1-4.5 4.5h-9l-6 5v-5h-1a4.5 4.5 0 0 1-4.5-4.5v-6A4.5 4.5 0 0 1 22 28z" fill="${bubble}"/>`;
const logo = `<rect width="64" height="64" rx="15" fill="#24403C"/>${glyph("#B9E0DA", "#F5C04A")}`;
// The glyph spans x 8..56, y 10..53; `scale` shrinks it around the centre.
const centred = (scale, content) => `<g transform="translate(32 32) scale(${scale}) translate(-32 -31.5)">${content}</g>`;
const icons = {
  "icon-192.png": [192, `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">${logo}</svg>`],
  "icon-512.png": [512, `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">${logo}</svg>`],
  // Android masks may cut up to 20% on each side.
  "maskable-512.png": [512, `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" fill="#24403C"/>${centred(0.62, glyph("#B9E0DA", "#F5C04A"))}</svg>`],
  // iOS rounds the corners itself and shows transparency as black.
  "apple-touch-icon.png": [180, `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" fill="#24403C"/>${centred(0.78, glyph("#B9E0DA", "#F5C04A"))}</svg>`],
  // Android draws the status-bar badge from the alpha channel only.
  "badge-96.png": [96, `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">${centred(0.9, glyph("#fff", "#fff"))}</svg>`],
};
const browser = await chromium.launch({ executablePath: process.env.CHROME });
for (const [name, [size, svg]] of Object.entries(icons)) {
  const page = await browser.newPage({ viewport: { width: size, height: size } });
  await page.setContent(`<body style="margin:0;background:transparent">${svg.replace("<svg ", `<svg width="${size}" height="${size}" `)}</body>`);
  writeFileSync(`${out}/${name}`, await page.screenshot({ omitBackground: true }));
  await page.close();
}
await browser.close();
