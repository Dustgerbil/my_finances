export interface Account {
  id: string;
  name: string;
  type: string;
  institution: string;
  currency: string;
  is_manual: boolean;
  is_ignored: boolean;
  interest_rate: number | null;
  repayment_amount: number | null;
  repayment_frequency: number | null;
  repayment_frequency_unit: "days" | "weeks" | "months" | null;
  rate_expiry: string | null;
  initial_next_repayment_date: string | null;
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
  is_retired: boolean;
  account: Account;
  latest_balance: number | null;
  previous_balance: number | null;
  latest_date: string | null;
  max_balance: number | null;
}

export interface AccountHistory {
  asset_financing: AssetFinancing | null;
  is_retired: boolean;
  account: Account;
  values: AccountValue[];
  current_balance: number | null;
  previous_balance: number | null;
  min_balance: number | null;
  min_balance_date: string | null;
  max_balance: number | null;
  max_balance_date: string | null;
  next_repayment_date: string | null;
  estimated_repayment_date: string | null;
  repayment_estimate_status: string | null;
}

export interface FinancingLiability {
  id: string;
  name: string;
  balance: number | null;
  recorded_date: string | null;
  is_retired: boolean;
  is_ignored: boolean;
}

export interface AssetFinancing {
  linked_liabilities: FinancingLiability[];
  available_liabilities: FinancingLiability[];
  total_debt: number | null;
  equity_amount: number | null;
  equity_percent: number | null;
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

export interface DataHealth {
  db_path: string;
  is_icloud: boolean;
  integrity_status: string;
  last_fetch_at: string | null;
  last_backup_at: string | null;
  account_count: number;
  value_count: number;
  pending_conflict_copies: number;
}
