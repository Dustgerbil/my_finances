# Linked assets across providers

On the primary asset's detail page, use **Linked assets → Link assets**, select
previous provider accounts, and save. Retired and ignored accounts are eligible.
The primary asset defaults to combined balances, history, minimum/maximum and
period changes. **Show this asset only** switches those figures temporarily;
**Show combined assets** switches back. Dashboard, Retired Accounts and Other
Accounts cards offer the same view control. Opening a page again resets its view.
Linked provider accounts retain their own pages and link back to the primary.

## Calculation and storage

- A group is one primary asset plus its explicitly linked accounts. Nested links,
  cycles, duplicate selections and membership in multiple groups are rejected in
  an atomic transaction. New links require real NZD assets with non-negative
  recorded balances; existing links remain removable after values disappear or
  become negative.
- Settings `asset_parent:<member id>` identify the primary asset. Empty values are
  unlink tombstones for the existing keep-live conflict merge. No schema change
  or dependency was added. The mobile snapshot allowlist includes these keys;
  iOS supports display and temporary toggling, with no mutation command registered.
- The combined series uses the union of recorded dates, selects the last row on
  each account/day and sums latest known values as of that date. Before a member's
  first record it contributes zero. If a member has no history at all, combined
  figures are unavailable. Earlier history cannot establish whether an unobserved
  account had an existing balance.
- Closing zero values must be recorded to stop old balances carrying forward.
  Retirement never implies zero. Asynchronous provider snapshots can show transfer
  spikes/dips; no transaction matching or guessed adjustments are applied.
- The shared series drives charts, extrema, direction, high badges and lookback
  changes. Insufficient lookback history remains unavailable; a zero baseline has
  a dollar change but no percentage. These are balance changes, not returns net
  of contributions, fees or withdrawals.
- Raw balances remain separate from combined presentation. Portfolio totals,
  breakdown charts, reports, account editing and retirement warnings retain their
  existing per-account scope. Combined equity sums the assets and each member's
  linked debts; liability editing still changes only the primary asset's links.
  The individual toggle also restores that asset's own debt and equity figures. Linking never rewrites recorded values or totals,
  ignores/retires an account, or duplicates it in the portfolio totals.

## Verification

Rust regression tests cover provider transitions and closing zeros, duplicate-day
rows, carried balances, retired/ignored links, missing history, raw totals,
lookbacks, zero baselines, negative balances, atomic rejection, cycles, shared
membership, unlinking/reassignment and read-only mobile snapshot preservation.
The browser fixture checks saving/cancelling links, scope switching, refreshed
charts, draft preservation, dashboard totals and read-only controls.
