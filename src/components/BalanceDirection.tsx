interface BalanceDirectionProps {
  current: number | null | undefined;
  previous: number | null | undefined;
}

export default function BalanceDirection({
  current,
  previous,
}: BalanceDirectionProps) {
  if (current == null || previous == null) return null;

  const direction = current > previous ? "up" : current < previous ? "down" : "flat";
  const symbol = direction === "up" ? "↑" : direction === "down" ? "↓" : "—";
  const label =
    direction === "up"
      ? "Increased since the previous saved value"
      : direction === "down"
        ? "Decreased since the previous saved value"
        : "Unchanged since the previous saved value";

  return (
    <span
      className={`balance-direction ${direction}`}
      role="img"
      aria-label={label}
      title={label}
    >
      {symbol}
    </span>
  );
}
