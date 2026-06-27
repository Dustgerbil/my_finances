export interface Account {
  id: string;
  name: string;
  type: string;
  institution: string;
  currency: string;
  is_manual: boolean;
  is_ignored: boolean;
}

export interface AccountValue {
  id: number;
  account_id: string;
  balance: number;
  recorded_date: string;
}

/** Return value of `delete_last_manual_value`: the row that was removed. */
export interface DeletedValue {
  balance: number;
  recorded_date: string;
}

export interface AccountSummary {
  account: Account;
  latest_balance: number | null;
  latest_date: string | null;
  max_balance: number | null;
}

export interface AccountHistory {
  account: Account;
  values: AccountValue[];
  current_balance: number | null;
  min_balance: number | null;
  max_balance: number | null;
}

export interface AccountChange {
  period: string;
  previous_balance: number | null;
  previous_date: string | null;
  change_amount: number | null;
  change_percent: number | null;
}

export interface BackupSettings {
  frequency: string; // "off" | "daily" | "weekly" | "monthly"
  last_backup_at: string | null; // ISO-8601 UTC, or null if never
  backup_dir: string; // resolved absolute path in use
  is_default_dir: boolean;
}
