# Asset-linked liabilities and equity

An asset can link multiple existing liabilities. Each liability belongs to at most
one asset; unlink it before assigning it elsewhere. Account Detail provides an
Edit / Save / Cancel flow and displays the latest linked balances and their dates.

Store assignments as `settings['liability_asset:<liability id>'] = <asset id>`.
An empty value is an explicit unlink tombstone, preserving the existing keep-live
conflict-merge behaviour. No schema migration or dependency is required.

Calculate equity in Rust from the latest recorded values: asset value minus the
sum of remaining debts (`max(-balance, 0)`). Percentage is equity / asset value
times 100, unavailable for zero or negative asset values. Missing balances make
the calculation unavailable. Negative equity is displayed without clamping.
Paid-off, retired, or ignored linked liabilities remain linked; credit balances
contribute zero debt. Links never alter balances, history, or virtual totals.

Export the financial settings to the iOS snapshot; retain read-only enforcement.
Validate multiple debts, latest-value ordering, repayment/zero/missing balances,
invalid assignments, atomic saves, reversible unlinking, total preservation, and
snapshot export with Rust tests. Build the frontend and signed macOS app.
