import { chromium } from 'playwright';
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 });
page.on('pageerror', e => console.log('PAGEERR', e.message));
await page.goto('http://localhost:5199/');
await page.waitForSelector('h1');
await page.waitForTimeout(6000);
// hover the second row (BOSS RUSH), which is not the cursor
await page.mouse.move(640, 522);
await page.waitForTimeout(400);
await page.screenshot({ path: '/tmp/b-hover.png', clip: { x: 370, y: 395, width: 545, height: 250 } });
await page.mouse.down();
await page.waitForTimeout(300);
await page.screenshot({ path: '/tmp/b-press.png', clip: { x: 370, y: 395, width: 545, height: 250 } });
await page.mouse.up();
await browser.close();
