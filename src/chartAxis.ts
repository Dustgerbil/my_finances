export type AdaptiveBalanceAxis = {
  domain: [number, number];
  ticks: number[];
  tickStep: number;
};

function niceTickStep(value: number) {
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const normalized = value / magnitude;
  const niceNormalized = normalized <= 1
    ? 1
    : normalized <= 2
    ? 2
    : normalized <= 2.5
    ? 2.5
    : normalized <= 5
    ? 5
    : 10;
  return niceNormalized * magnitude;
}

export function buildAdaptiveBalanceAxis(
  values: number[],
): AdaptiveBalanceAxis | null {
  if (values.length === 0) return null;

  const rawMin = Math.min(...values);
  const rawMax = Math.max(...values);
  const rawSpan = rawMax - rawMin;
  const distanceFromZero = rawMin > 0
    ? rawMin
    : rawMax < 0
    ? Math.abs(rawMax)
    : 0;
  // Retain a meaningful zero baseline, but drop it when it would consume most
  // of the plot without helping explain the visible movement.
  const includeZero = rawSpan > 0 && distanceFromZero <= rawSpan * 2;
  const magnitude = Math.max(Math.abs(rawMin), Math.abs(rawMax), 1);
  // Avoid exaggerating a flat or nearly flat series with an extremely tight axis.
  const minimumSpan = Math.max(magnitude * 0.005, 100);

  let paddedMin: number;
  let paddedMax: number;
  if (includeZero && rawMin >= 0) {
    paddedMin = 0;
    paddedMax = rawMax * 1.12;
  } else if (includeZero && rawMax <= 0) {
    paddedMin = rawMin * 1.12;
    paddedMax = 0;
  } else {
    const midpoint = (rawMin + rawMax) / 2;
    // Add 12% visual breathing room above and below the effective data span.
    const paddedSpan = Math.max(rawSpan, minimumSpan) * 1.24;
    paddedMin = midpoint - paddedSpan / 2;
    paddedMax = midpoint + paddedSpan / 2;
  }

  const tickStep = niceTickStep((paddedMax - paddedMin) / 6);
  const domainMin = includeZero && rawMin >= 0
    ? 0
    : Math.floor(paddedMin / tickStep) * tickStep;
  const domainMax = includeZero && rawMax <= 0
    ? 0
    : Math.ceil(paddedMax / tickStep) * tickStep;
  const ticks: number[] = [];
  for (let tick = domainMin; tick <= domainMax + tickStep / 2; tick += tickStep) {
    ticks.push(Number(tick.toFixed(10)));
  }

  return {
    domain: [domainMin, domainMax],
    ticks,
    tickStep,
  };
}

export function formatAdaptiveAxisTick(value: number, tickStep: number) {
  const abs = Math.abs(value);
  const sign = value < 0 ? "-" : "";
  if (abs >= 1000) {
    const stepInThousands = tickStep / 1000;
    const fractionDigits = stepInThousands >= 1
      ? 0
      : stepInThousands >= 0.1
      ? 1
      : 2;
    return `${sign}$${(abs / 1000).toFixed(fractionDigits)}k`;
  }

  const fractionDigits = tickStep >= 1 ? 0 : 2;
  return `${sign}$${abs.toFixed(fractionDigits)}`;
}
