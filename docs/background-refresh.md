# Background updates without interrupting edits

Database file notifications (`db-changed`) can come from local saves or iCloud.
Previously, each notification used the initial loading screen, removing the
account page or card grid and collapsing its scroll position. Account Detail also
reset repayment drafts on every history response; remounting the linked-liability
component discarded its editor state.

`useLiveData` now shares the refresh policy across Dashboard, Account Detail,
Other Accounts and Retired Accounts:

- Show loading only on initial entry; retain existing data during refresh or error.
- Coalesce database events and wait until interaction has stopped for 1.5 seconds.
- Pause while a form control has focus, the app is hidden, or an account draft/save
  is active. A nonempty manual value remains protected after it loses focus.
- Resume queued updates after editing ends. Save actions refresh immediately.
- Recheck interaction after asynchronous reads and reject superseded responses.
- Keep identical results unchanged and clean up listeners on navigation.
- Reset account-local drafts when navigating to another account.

No database, financial calculations, or sync/publishing schedule is changed.

## Regression check

Start the Vite preview with `npm run dev -- --host 127.0.0.1 --port 1420`.
Run `node scripts/test-background-refresh.cjs` with an existing Playwright runtime
available through Node's module resolution (or `NODE_PATH`). `TEST_BROWSER` can
select an already-installed Chromium executable; `TEST_ORIGIN` overrides the URL.
The test supplies synthetic Tauri responses and emits database events in the
browser. It does not access the live finance database or bank services.

Checks cover manual values, focus/scroll retention, linked-liability Save/Cancel,
repayment drafts, interaction during in-flight reads, refresh failure/recovery,
and scroll retention on all account-list screens. Native iCloud notification
delivery is outside the browser test's scope.
