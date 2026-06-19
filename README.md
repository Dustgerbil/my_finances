# My Finances

A local, private desktop app to review and manage personal finances on macOS.
One button fetches current account balances from [Akahu](https://akahu.io/)
(a New Zealand open-banking aggregator), stores them in a local SQLite
database, and shows summaries plus per-account history graphs.

## Why

Banks and accounting apps show your money scattered across dozens of screens
and logins. This app pulls every NZD account you've connected to Akahu into one
view: a dashboard of cards with current balances, all-time-high badges,
sub-totals for assets and liabilities, and a line chart of each account's
history. Your data never leaves your machine (except the one fetch from Akahu)
and never touches a third-party server.

## Features

- **One-click balance refresh** — fetches live balances from all your Akahu
  accounts and stores them once per day.
- **Dashboard** — a card per account showing current balance and 30/180/360-day
  change chips, plus virtual "Total", "Assets & Savings", and "Liabilities"
  sub-total cards.
- **All-time-high badge (⭐)** — a card earns a star when its latest balance
  equals its all-time maximum.
- **Per-account history** — click any card for a line chart of the account over
  time, with current, min, and max values.
- **Accounts config** — hide accounts you don't care about (they're excluded
  from totals too), and add manual accounts (property, cash, etc.) with
  hand-entered values.
- **Settings** — store your Akahu credentials in the macOS keychain, and toggle
  automatic fetch on app launch.
- **iCloud sync (multi-machine)** — the database can live in iCloud Drive so the
  same dataset is available across your Macs, with lossless conflict merging
  when two machines write while apart.

> **Currency:** only NZD accounts are stored; non-NZD accounts are silently
> ignored.

## Tech stack

- **Frontend:** Vite 7, React 19, TypeScript, recharts, React Router v7
- **Desktop shell:** Tauri v2
- **Storage:** SQLite via `rusqlite`
- **HTTP:** `reqwest` for Akahu calls
- **Secrets:** macOS keychain via `keyring` (falls back to env vars
  `AKAHU_APP_ID` / `AKAHU_USER_TOKEN`)

## Prerequisites

- macOS (Apple Silicon or Intel)
- [Node.js](https://nodejs.org/) 20+ and npm
- [Rust](https://www.rust-lang.org/tools/install) (stable)
- An [Akahu](https://akahu.io/) account with at least one bank connection, plus
  an app ID and user token (create them in the Akahu developer dashboard and
  paste them into the app's Settings screen, or set the env vars above).

## Build & run

```bash
npm install            # first time only
npm run tauri dev      # dev mode (hot reload, opens a window)
npm run tauri build    # production build -> My Finances.app
```

The built app lands at
`src-tauri/target/release/bundle/macos/My Finances.app`.

## Where your data lives

- By default: `~/Library/Application Support/my_finances.db`
- If iCloud Drive is available, the database is stored in
  `~/Library/Mobile Documents/com~apple~CloudDocs/my_finances/my_finances.db`
  so it syncs across your Macs. The first launch copies any existing local
  database into iCloud once (the old local copy is kept as a backup).

Inspect the database directly with:

```bash
sqlite3 ~/Library/Application\ Support/my_finances.db
# or, if using iCloud:
sqlite3 ~/Library/Mobile\ Documents/com~apple~CloudDocs/my_finances/my_finances.db
```

## Project layout

```
src/                React 19 frontend
  App.tsx           Routes
  pages/            Dashboard, AccountDetail, AccountsConfig, Settings
src-tauri/          Rust backend (single-file src/lib.rs)
```

## License

Personal project — not currently licensed for redistribution. All rights
reserved.
