// Synthetic browser checks; uses the shared Playwright runtime, no new dependency.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const origin = process.env.TEST_ORIGIN || 'http://127.0.0.1:15380';
(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.TEST_BROWSER });
  const page = await browser.newPage({ viewport: { width: 1000, height: 800 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(`${origin}/`, async route => {
    const response = await route.fetch();
    const html = await response.text();
    await route.fulfill({ response, body: html.replace(/<script type="module" src="\/src\/main\.tsx[^"]*"><\/script>/, `<script type="module">
      import { mockIPC } from '/node_modules/@tauri-apps/api/mocks.js';
      import { emit } from '/node_modules/@tauri-apps/api/event.js';
      window.fixture = { linked: [], writes: 0, reads: 0, readOnly: false, failSave: false };
      window.changed = () => emit('db-changed');
      const account = id => ({ id, name: id === 'new' ? 'New Provider' : id === 'old' ? 'Old Provider' : 'Total', type: 'INVESTMENT', institution: 'Test', currency: 'NZD', is_manual: true, is_ignored: false });
      const old = { id: 'old', name: 'Old Provider', balance: 50, recorded_date: '2026-10-16', is_retired: true, is_ignored: false };
      const combined = () => ({ account_count: 2, latest_balance: 250, previous_balance: 240, latest_date: '2026-10-16', max_balance: 250 });
      const summary = id => ({ account: account(id), is_retired: id === 'old', latest_balance: id === 'new' ? 200 : id === 'old' ? 50 : 250, previous_balance: 190, latest_date: '2026-10-16', max_balance: 200, combined: id === 'new' && fixture.linked.length ? combined() : null });
      function history(args) {
        const linked = fixture.linked.length > 0;
        const joined = linked && args.includeLinked !== false;
        const balances = joined ? [100,240,250] : [0,190,200];
        return { account: account('new'), is_retired: false, current_balance: balances[2], previous_balance: balances[1], min_balance: balances[0], min_balance_date: '2026-01-01', max_balance: balances[2], max_balance_date: '2026-10-16', asset_financing: { linked_liabilities: [{ ...old, id: 'loan', name: 'This asset loan', balance: -30 }], available_liabilities: [], total_debt: 30, equity_amount: 170, equity_percent: 85 }, combined_financing: joined ? { linked_liabilities: [{ ...old, id: 'loan', name: 'This asset loan', balance: -30 }, { ...old, id: 'loan2', name: 'Other asset loan', balance: -40 }], available_liabilities: [], total_debt: 70, equity_amount: 180, equity_percent: 72 } : null,
          values: balances.map((balance,i) => ({ id: i, account_id: 'new', balance, recorded_date: ['2026-01-01','2026-10-15','2026-10-16'][i] })),
          asset_links: { linked_assets: linked ? [old] : [], available_assets: linked ? [] : [old], primary_asset: null } };
      }
      mockIPC(async (cmd,args) => {
        if (cmd === 'get_sync_status' || cmd === 'sync_mobile_snapshot') return { read_only: sessionStorage.getItem('readOnly') === 'true', has_cache: true };
        if (cmd === 'get_autofetch_setting' || cmd === 'has_akahu_credentials') return false;
        if (cmd === 'get_account_history') { fixture.reads++; return history(args); }
        if (cmd === 'get_account_changes') return [{ period: 'all', change_amount: fixture.linked.length && args.includeLinked !== false ? 150 : 200, change_percent: fixture.linked.length && args.includeLinked !== false ? 150 : null }];
        if (cmd === 'set_linked_assets') { fixture.writes++; if (fixture.failSave) throw new Error('Save failed'); fixture.linked = args.linkedAssetIds; await changed(); return; }
        if (cmd === 'get_accounts_summary') return ['__total__','__assets_total__','new','old'].map(summary);
        if (cmd === 'get_other_accounts_summary') return [summary('new')];
        return null;
      }, { shouldMockEvents: true });
      await import('/src/main.tsx');
    </script>`) });
  });
  const open = () => page.goto(`${origin}/#/account/new`);
  const stat = index => page.locator('.detail-stats').first().locator('.stat-value').nth(index).innerText();
  try {
    await open();
    await page.getByRole('button', { name: 'Link assets', exact: true }).click();
    const choice = page.locator('.linked-assets input[type=checkbox]');
    await choice.check();
    const reads = await page.evaluate(() => fixture.reads);
    await page.evaluate(() => { document.activeElement.blur(); window.changed(); });
    await page.waitForTimeout(2300);
    assert.equal(await choice.isChecked(), true);
    assert.equal(await page.evaluate(() => fixture.reads), reads, 'asset draft blocks refresh');
    await page.locator('.linked-assets').getByRole('button', { name: 'Cancel', exact: true }).click();
    assert.equal(await page.evaluate(() => fixture.writes), 0);
    await page.getByRole('button', { name: 'Link assets', exact: true }).click();
    assert.equal(await choice.isChecked(), false);
    await choice.check();
    await page.evaluate(() => fixture.failSave = true);
    await page.getByRole('button', { name: 'Save linked assets', exact: true }).click();
    await page.getByRole('alert').filter({ hasText: 'Save failed' }).waitFor();
    assert.equal(await choice.isChecked(), true, 'failed save preserves selection');
    await page.evaluate(() => fixture.failSave = false);
    await page.getByRole('button', { name: 'Save linked assets', exact: true }).click();
    await page.getByText('Combined · 2 accounts', { exact: true }).waitFor();
    assert.match(await stat(0), /250\.00/);
    assert.match(await stat(1), /100\.00/);
    assert.match(await stat(2), /250\.00/);
    assert.match(await page.locator('.changes-grid').innerText(), /150\.00/);
    await page.waitForTimeout(1800);
    assert.match(await page.locator('.equity-stats').innerText(), /180\.00/);
    const combinedChart = await page.locator('.recharts-line-curve').getAttribute('d');
    const writes = await page.evaluate(() => fixture.writes);
    await page.locator('#addValue').fill('123');
    await page.getByRole('button', { name: 'Show this asset only', exact: true }).click();
    assert.match(await stat(0), /200\.00/);
    assert.match(await page.locator('.equity-stats').innerText(), /170\.00/);
    assert.match(await stat(1), /0\.00/);
    assert.equal(await page.locator('#addValue').inputValue(), '123', 'toggle preserves manual draft');
    assert.match(await page.locator('.changes-grid').innerText(), /200\.00/);
    await page.waitForTimeout(1800);
    assert.notEqual(await page.locator('.recharts-line-curve').getAttribute('d'), combinedChart, 'chart changes with scope');
    await page.getByRole('button', { name: 'Show combined assets', exact: true }).click();
    assert.match(await stat(0), /250\.00/);
    assert.equal(await page.evaluate(() => fixture.writes), writes, 'view toggles never write');
    await page.locator('#addValue').fill('');
    await page.waitForTimeout(1800);
    await page.screenshot({ path: '/tmp/linked-assets-detail.png', fullPage: true });
    await page.getByRole('button', { name: 'Back to Dashboard', exact: false }).click();
    const primary = page.getByRole('button', { name: 'View New Provider account details', exact: true });
    await primary.waitFor();
    assert.match(await primary.innerText(), /250\.00/);
    const total = page.getByRole('button', { name: 'View Total account details', exact: true }).first();
    assert.match(await total.innerText(), /250\.00/);
    assert.equal(await page.getByRole('button', { name: /Show this asset only|Show combined assets/ }).count(), 0, 'home screen has no asset view toggles');
    await primary.click();
    await page.getByText('Combined · 2 accounts', { exact: true }).waitFor();
    await page.getByRole('button', { name: 'Edit linked assets', exact: true }).click();
    await choice.uncheck();
    await page.getByRole('button', { name: 'Save linked assets', exact: true }).click();
    await page.getByRole('button', { name: 'Link assets', exact: true }).waitFor();
    assert.equal(await page.locator('.asset-view-controls').count(), 0);
    assert.match(await stat(0), /200\.00/);
    // Re-open a read-only reader with linked fixture data.
    await page.evaluate(() => { sessionStorage.setItem('readOnly','true'); });
    await page.reload();
    await page.getByRole('heading', { name: 'New Provider', exact: false }).waitFor();
    await page.evaluate(() => { fixture.linked = ['old']; window.changed(); });
    await page.getByText('Combined · 2 accounts', { exact: true }).waitFor();
    assert.equal(await page.getByRole('button', { name: 'Edit linked assets', exact: true }).count(), 0);
    assert.equal(await page.locator('#addValue').count(), 0);
    await page.getByRole('button', { name: 'Show this asset only', exact: true }).click();
    assert.match(await stat(0), /200\.00/);
    assert.equal(await page.evaluate(() => fixture.writes), 0);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(2100);
    await page.screenshot({ path: '/tmp/linked-assets-phone.png', fullPage: true });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'no phone-width horizontal overflow');
    assert.deepEqual(errors, []);
    console.log('PASS linked asset editing, save failure, Cancel, draft refresh protection, combined stats/chart/changes, temporary toggles, raw portfolio totals, unlink, read-only and narrow layout');
  } catch (error) {
    await page.screenshot({ path: '/tmp/linked-assets-failure.png', fullPage: true });
    throw error;
  } finally { await browser.close(); }
})();
