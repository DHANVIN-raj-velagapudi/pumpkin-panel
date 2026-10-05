// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
import { useId } from "react";

/**
 * A compact area chart for a single series.
 *
 * Hand-drawn SVG rather than a charting library: the whole panel ships inside
 * one binary, and this is a few hundred bytes instead of a few hundred
 * kilobytes.
 */
export default function Chart({
  values,
  max,
  color = "var(--accent)",
  height = 64,
  format,
}: {
  values: number[];
  /** Upper bound of the y axis. When omitted, scales to the data. */
  max?: number;
  color?: string;
  height?: number;
  format: (value: number) => string;
}) {
  const gradientId = useId();
  const width = 300;

  if (values.length === 0) {
    return (
      <div className="chart-empty" style={{ height }}>
        Waiting for samples…
      </div>
    );
  }

  const peak = Math.max(max ?? 0, ...values, 0.0001);
  // A single sample has no width to draw across, so mirror it into a flat line.
  const points = values.length === 1 ? [values[0], values[0]] : values;
  const step = width / (points.length - 1);

  const coords = points.map((value, index) => {
    const x = index * step;
    const y = height - (value / peak) * (height - 4) - 2;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });

  const line = `M ${coords.join(" L ")}`;
  const area = `${line} L ${width},${height} L 0,${height} Z`;

  const latest = values[values.length - 1];

  return (
    <div className="chart">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        width="100%"
        height={height}
        role="img"
        aria-label={`Latest ${format(latest)}`}
      >
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={color} stopOpacity="0.35" />
            <stop offset="100%" stopColor={color} stopOpacity="0" />
          </linearGradient>
        </defs>
        <path d={area} fill={`url(#${gradientId})`} />
        <path
          d={line}
          fill="none"
          stroke={color}
          strokeWidth="1.5"
          strokeLinejoin="round"
          strokeLinecap="round"
          vectorEffect="non-scaling-stroke"
        />
      </svg>
      <div className="chart-axis">
        <span>{format(0)}</span>
        <span>{format(peak)}</span>
      </div>
    </div>
  );
}
