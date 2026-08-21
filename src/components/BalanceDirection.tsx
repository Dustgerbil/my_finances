interface BalanceDirectionProps {
  current: number | null | undefined;
  previous: number | null | undefined;
}

export default function BalanceDirection({
  current,
  previous,
}: BalanceDirectionProps) {
  if (current == null || previous == null || current === previous) return null;

  const direction = current > previous ? "up" : "down";
  const symbol = direction === "up" ? "↑" : "↓";
  const label =
    direction === "up"
      ? "Increased since the previous saved value"
      : "Decreased since the previous saved value";

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
