// Run against `npm run dev` using an existing Playwright installation.
// NODE_PATH may point to the shared runtime; no project dependency is required.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const origin = process.env.TEST_ORIGIN || 'http://127.0.0.1:1420';

(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.TEST_BROWSER });
  const page = await browser.newPage({ viewport: { width: 1000, height: 650 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(`${origin}/`, async route => {
    const response = await route.fetch();
    const html = await response.text();
    await route.fulfill({ response, body: html.replace('<script type="module" src="/src/main.tsx"></script>', `<script type="module">
      import { mockIPC } from '/node_modules/@tauri-apps/api/mocks.js';
      import { emit } from '/node_modules/@tauri-apps/api/event.js';
      const account = (id) => ({ id, name: id === 'house' ? 'Test House' : 'Test Mortgage', type: 'PROPERTY', institution: 'Test', currency: 'NZD', is_manual: true, is_ignored: false, interest_rate: 5, repayment_amount: 1000, repayment_frequency: 1, repayment_frequency_unit: 'months', rate_expiry: null, initial_next_repayment_date: '2026-11-01' });
      const loan = { id: 'loan', name: 'Test Mortgage', balance: -200000, recorded_date: '2026-10-11', is_retired: false, is_ignored: false };
      window.fixture = { reads: 0, delay: 0, fail: false, balance: 800000, linked: [], rate: 5 };
      window.changed = () => emit('db-changed');
      function history(id) {
        const balance = id === 'house' ? fixture.balance : -200000;
        return { account: { ...account(id), interest_rate: fixture.rate }, is_retired: false, current_balance: balance, previous_balance: balance - 100, min_balance: balance - 100, max_balance: balance, min_balance_date: '2026-10-01', max_balance_date: '2026-10-11', next_repayment_date: '2026-11-01', estimated_repayment_date: null, repayment_estimate_status: null,
          values: Array.from({ length: 10 }, (_, i) => ({ id: i, account_id: id, balance: balance - 100 + i * 10, recorded_date: '2026-10-' + String(i + 1).padStart(2, '0') })),
          asset_financing: id === 'house' ? { linked_liabilities: fixture.linked.length ? [loan] : [], available_liabilities: fixture.linked.length ? [] : [loan], total_debt: fixture.linked.length ? 200000 : null, equity_amount: fixture.linked.length ? balance - 200000 : null, equity_percent: fixture.linked.length ? 75 : null } : null };
      }
      mockIPC(async (cmd, args) => {
        if (cmd === 'get_sync_status') return { read_only: false, has_cache: true };
        if (cmd === 'get_autofetch_setting' || cmd === 'has_akahu_credentials') return false;
        if (cmd === 'get_account_history') {
          fixture.reads++;
          const result = history(args.accountId);
          if (fixture.delay) await new Promise(resolve => setTimeout(resolve, fixture.delay));
          if (fixture.fail) throw new Error('Test read failure');
          return result;
        }
        if (cmd === 'get_account_changes') return [{ period: 'all', change_amount: 100, change_percent: 1, previous_balance: 799900, previous_date: '2026-10-01' }];
        if (cmd === 'set_asset_liabilities') { fixture.linked = args.liabilityIds; await changed(); return; }
        if (cmd === 'add_manual_account_value') { fixture.balance = args.request.balance; await changed(); return; }
        if (cmd === 'update_liability_repayment') { fixture.rate = args.request.interest_rate; await changed(); return; }
        if (cmd === 'get_accounts_summary' || cmd === 'get_other_accounts_summary') return Array.from({ length: 24 }, (_, i) => ({ account: { ...account('house'), id: 'house' + i, name: 'Test account ' + i }, is_retired: location.hash.includes('retired-accounts'), latest_balance: fixture.balance, previous_balance: 700000, latest_date: '2026-10-11', max_balance: fixture.balance }));
        return null;
      }, { shouldMockEvents: true });
      await import('/src/main.tsx');
    </script>`) });
  });
  const emit = () => page.evaluate(() => window.changed());
  const idle = () => page.waitForTimeout(2200);
  const blur = () => page.evaluate(() => document.activeElement?.blur());
  const reads = () => page.evaluate(() => fixture.reads);
  const open = async id => {
    await page.goto(`${origin}/#/account/${id}`);
    await page.getByRole('heading', { name: id === 'house' ? 'Test House' : 'Test Mortgage', exact: false }).waitFor();
    await page.waitForTimeout(150);
  };
  try {
    await open('house');
    const input = page.locator('#addValue');
    await input.fill('812345');
    const initialReads = await reads();
    const before = await page.evaluate(() => { window.savedInput = document.querySelector('#addValue'); return window.scrollY; });
    await emit(); await idle();
    assert.equal(await input.inputValue(), '812345');
    assert.equal(await reads(), initialReads, 'draft blocks background reads');
    assert.deepEqual(await page.evaluate(() => ({ same: savedInput === document.querySelector('#addValue'), focused: savedInput === document.activeElement, scroll: window.scrollY })), { same: true, focused: true, scroll: before });
    await blur(); await idle();
    assert.equal(await reads(), initialReads, 'blur does not discard an unsaved draft');
    await page.locator('.add-value-form').getByRole('button', { name: 'Save', exact: true }).click();
    await page.getByText('Value added', { exact: true }).waitFor();
    assert.equal(await input.inputValue(), '');
    assert.equal(await page.evaluate(() => fixture.balance), 812345);
    assert.equal(await page.evaluate(() => savedInput === document.querySelector('#addValue')), true, 'save preserves mounted form');
    console.log('PASS manual draft, focus, scroll and save');

    await page.getByRole('button', { name: 'Add liabilities', exact: true }).click();
    const choice = page.locator('.liability-options input');
    await choice.check(); await blur();
    const linkReads = await reads();
    await emit(); await idle();
    assert.equal(await choice.isChecked(), true);
    assert.equal(await reads(), linkReads);
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await idle();
    assert.ok(await reads() > linkReads, 'queued change resumes after Cancel');
    await page.getByRole('button', { name: 'Add liabilities', exact: true }).click();
    assert.equal(await choice.isChecked(), false, 'Cancel discards selection');
    await choice.check();
    await page.locator('.asset-liabilities').getByRole('button', { name: 'Save', exact: true }).click();
    await page.locator('.linked-liabilities-list').waitFor();
    assert.deepEqual(await page.evaluate(() => fixture.linked), ['loan']);
    console.log('PASS linked liabilities stay open; Cancel and Save work');

    await open('loan');
    await page.getByRole('button', { name: 'Edit', exact: true }).click();
    await page.locator('#interestRate').fill('6.75'); await blur();
    await page.evaluate(() => { fixture.rate = 4; });
    await emit(); await idle();
    assert.equal(await page.locator('#interestRate').inputValue(), '6.75');
    await page.locator('.liability-repayment-form').getByRole('button', { name: 'Save repayment details', exact: true }).click();
    await page.locator('.liability-repayment-form').getByRole('button', { name: 'Edit', exact: true }).waitFor();
    assert.equal(await page.evaluate(() => fixture.rate), 6.75);
    console.log('PASS repayment draft survives a competing update');

    await open('house'); await idle();
    const beforeRace = await page.locator('.stat-current > span').first().innerText();
    await page.evaluate(() => { fixture.delay = 1100; fixture.balance = 900000; });
    const raceReads = await reads();
    await emit();
    await page.waitForFunction(n => fixture.reads > n, raceReads);
    await page.locator('#addValue').fill('888888');
    await idle();
    assert.equal(await page.locator('#addValue').inputValue(), '888888');
    assert.equal(await page.locator('.stat-current > span').first().innerText(), beforeRace, 'in-flight result deferred');
    await page.evaluate(() => { fixture.delay = 0; });
    await page.locator('.add-value-form').getByRole('button', { name: 'Save', exact: true }).click();
    await page.getByText('Value added', { exact: true }).waitFor();
    await idle();
    assert.equal(await page.locator('.stat-current > span').first().innerText(), '$888,888.00');
    console.log('PASS edits started during an in-flight refresh; saved value wins');

    await blur(); await idle();
    const staleReads = await reads();
    await page.evaluate(() => { fixture.delay = 3000; fixture.balance = 777777; });
    await emit();
    await page.waitForFunction(n => fixture.reads > n, staleReads);
    await page.locator('#addValue').fill('999999');
    await page.evaluate(() => { fixture.delay = 0; });
    await page.locator('.add-value-form').getByRole('button', { name: 'Save', exact: true }).click();
    await page.waitForFunction(() => fixture.balance === 999999);
    await page.waitForTimeout(3300);
    assert.equal(await page.locator('.stat-current > span').first().innerText(), '$999,999.00', 'late background response cannot overwrite a save');
    console.log('PASS stale response cannot overwrite newer saved data');

    await blur(); await idle();
    await page.evaluate(() => { fixture.fail = true; });
    await emit(); await idle();
    await page.getByRole('alert').waitFor();
    assert.equal(await page.locator('#addValue').count(), 1, 'read failure retains screen');
    await page.evaluate(() => { fixture.fail = false; });
    await emit(); await idle();
    assert.equal(await page.getByRole('alert').count(), 0, 'next successful refresh clears error');
    console.log('PASS background failure retains screen and recovers');

    for (const route of ['/', '/other-accounts', '/retired-accounts']) {
      await page.goto(`${origin}/#${route}`);
      await page.getByText('Test account 20', { exact: false }).waitFor();
      await page.evaluate(() => window.scrollTo(0, 800));
      const y = await page.evaluate(() => { window.savedCard = [...document.querySelectorAll('.account-name')].find(el => el.textContent.includes('Test account 20')); return window.scrollY; });
      await emit();
      for (let i = 0; i < 5; i++) {
        await page.evaluate(() => document.dispatchEvent(new Event('scroll')));
        await page.waitForTimeout(350);
      }
      await idle();
      assert.equal(await page.evaluate(() => window.scrollY), y);
      assert.equal(await page.evaluate(() => savedCard.isConnected), true);
    }
    assert.deepEqual(errors, []);
    console.log('PASS dashboard, other and retired pages preserve scroll and mounted cards');
  } catch (error) {
    console.error(await page.locator("body").innerText(), errors);
    throw error;
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
