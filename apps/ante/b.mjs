import { chromium } from 'playwright';
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 });
page.on('pageerror', e => console.log('PAGEERR', e.message));
await page.goto('http://localhost:5199/storybook-shim.html').catch(() => {});
await page.goto('http://localhost:5199/');
await page.waitForSelector('h1');
await page.waitForTimeout(6000);
await page.keyboard.press('Enter');
await page.keyboard.press('Enter');
await page.waitForTimeout(4000);
// Drive to a settled vault, then the shop.
for (let i = 0; i < 40; i += 1) {
  await page.keyboard.press('p');
  await page.waitForTimeout(120);
  await page.keyboard.press('Space');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(500);
  await page.keyboard.press('Enter');
  await page.waitForTimeout(500);
  await page.keyboard.press('Enter');
  await page.waitForTimeout(900);
  const shot = await page.screenshot({ path: '/tmp/b-probe.png' });
  if (i > 3) { await page.keyboard.press('Enter'); await page.waitForTimeout(900); }
  const png = await page.screenshot();
  // crude: look for the shop by sampling; just save every few iterations
  if (i % 4 === 3) await page.screenshot({ path: `/tmp/b-iter${i}.png` });
}
await page.screenshot({ path: '/tmp/b-shop.png' });
await browser.close();
