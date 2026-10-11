/** Calendar days until a stored YYYY-MM-DD expiry, independent of DST. */
export function daysUntilRateExpiry(expiry: string | null, today = new Date()): number | null {
  if (!expiry || !/^\d{4}-\d{2}-\d{2}$/.test(expiry)) return null;
  const [year, month, day] = expiry.split("-").map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) {
    return null;
  }
  const todayUtc = Date.UTC(today.getFullYear(), today.getMonth(), today.getDate());
  return (date.getTime() - todayUtc) / 86_400_000;
}
