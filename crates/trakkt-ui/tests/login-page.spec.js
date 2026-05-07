// Playwright test: verify login page renders correctly
const { chromium } = require('playwright');
const path = require('path');

const SCREENSHOT_DIR = path.join(__dirname, 'screenshots');
const BASE_URL = 'http://localhost:8099';

(async () => {
  const fs = require('fs');
  fs.mkdirSync(SCREENSHOT_DIR, { recursive: true });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await context.newPage();

  // Collect console messages and errors
  const consoleMessages = [];
  const pageErrors = [];
  page.on('console', msg => consoleMessages.push(`[${msg.type()}] ${msg.text()}`));
  page.on('pageerror', err => pageErrors.push(err.message));

  console.log('=== Test 1: Navigate to / should redirect to /login ===');
  await page.goto(BASE_URL + '/', { waitUntil: 'networkidle' });
  const url = page.url();
  console.log(`Current URL: ${url}`);
  if (url.includes('/login')) {
    console.log('PASS: Redirected to /login');
  } else {
    console.log('FAIL: Did not redirect to /login');
  }

  // Wait for WASM to initialize
  await page.waitForTimeout(3000);
  await page.screenshot({ path: path.join(SCREENSHOT_DIR, '01-login-page.png'), fullPage: true });
  console.log('Screenshot: 01-login-page.png');

  console.log('\n=== Test 2: Check login form elements ===');
  const emailInput = await page.$('input[type="email"]');
  const passwordInput = await page.$('input[type="password"]');
  const signInButton = await page.$('button[type="submit"]');

  console.log(`Email input: ${emailInput ? 'FOUND' : 'MISSING'}`);
  console.log(`Password input: ${passwordInput ? 'FOUND' : 'MISSING'}`);
  console.log(`Sign in button: ${signInButton ? 'FOUND' : 'MISSING'}`);

  if (signInButton) {
    const btnText = await signInButton.textContent();
    console.log(`Button text: "${btnText.trim()}"`);
  }

  // Check for labels
  const labels = await page.$$('label');
  console.log(`Labels found: ${labels.length}`);
  for (const label of labels) {
    const text = await label.textContent();
    console.log(`  - "${text.trim()}"`);
  }

  // Check for "Create an account" link
  const createAccountLink = await page.$('text=Create an account');
  console.log(`Create account link: ${createAccountLink ? 'FOUND' : 'MISSING'}`);

  // Check for "Can't sign in?" link
  const cantSignInLink = await page.$('text=Can\'t sign in?');
  console.log(`"Can't sign in?" link: ${cantSignInLink ? 'FOUND' : 'MISSING'}`);

  console.log('\n=== Test 3: Check page title and heading ===');
  const heading = await page.$('h1, h2, h3, [class*="text-2xl"], [class*="text-3xl"]');
  if (heading) {
    const headingText = await heading.textContent();
    console.log(`Heading: "${headingText.trim()}"`);
  } else {
    console.log('No heading found');
  }

  console.log('\n=== Test 4: Navigate to /settings/profile ===');
  await page.goto(BASE_URL + '/settings/profile', { waitUntil: 'networkidle' });
  await page.waitForTimeout(2000);
  await page.screenshot({ path: path.join(SCREENSHOT_DIR, '02-settings-profile.png'), fullPage: true });
  console.log(`Settings URL: ${page.url()}`);
  console.log('Screenshot: 02-settings-profile.png');

  console.log('\n=== Test 5: Navigate to /signup ===');
  await page.goto(BASE_URL + '/signup', { waitUntil: 'networkidle' });
  await page.waitForTimeout(2000);
  await page.screenshot({ path: path.join(SCREENSHOT_DIR, '03-signup-page.png'), fullPage: true });
  console.log(`Signup URL: ${page.url()}`);
  console.log('Screenshot: 03-signup-page.png');

  // Check signup form
  const signupEmailInput = await page.$('input[type="email"]');
  console.log(`Signup email input: ${signupEmailInput ? 'FOUND' : 'MISSING'}`);

  console.log('\n=== Console Messages ===');
  if (consoleMessages.length > 0) {
    consoleMessages.forEach(msg => console.log(`  ${msg}`));
  } else {
    console.log('  (none)');
  }

  console.log('\n=== Page Errors ===');
  if (pageErrors.length > 0) {
    pageErrors.forEach(err => console.log(`  ERROR: ${err}`));
  } else {
    console.log('  (none)');
  }

  // Summary
  console.log('\n=== SUMMARY ===');
  const allPassed = emailInput && passwordInput && signInButton && url.includes('/login');
  if (allPassed) {
    console.log('All critical checks PASSED - login page renders correctly');
  } else {
    console.log('Some checks FAILED - see details above');
  }

  await browser.close();
})();
