import { z } from "astro/zod";
import data from "../../../docs/benchmarks.json";

const schema = z.strictObject({
  provenance: z.strictObject({
    date: z.string(),
    carta: z.string(),
    pandoc: z.string(),
    hyperfine: z.string(),
    runner: z.string(),
    warmup: z.number(),
    runs: z.number(),
  }),
  timings: z
    .array(
      z.strictObject({
        surface: z.string(),
        group: z.string(),
        bytes: z.number(),
        runs: z.number(),
        carta_ms: z.number(),
        carta_sd_ms: z.number(),
        pandoc_ms: z.number(),
        pandoc_sd_ms: z.number(),
        // Null on hosts where `/usr/bin/time` cannot report peak memory.
        carta_rss: z.number().nullable(),
        pandoc_rss: z.number().nullable(),
      }),
    )
    .nonempty(),
  binary: z.strictObject({ carta: z.number(), pandoc: z.number() }),
});

const benchmarks = schema.parse(data);

export const { provenance, binary } = benchmarks;

export const results = benchmarks.timings.map((timing) => ({
  ...timing,
  speedup: timing.pandoc_ms / timing.carta_ms,
}));

export interface Range {
  low: number;
  high: number;
}

const round = (value: number) => Math.round(value * 10) / 10;

const range = (values: number[]): Range => ({
  low: round(Math.min(...values)),
  high: round(Math.max(...values)),
});

const speedups = (...surfaces: string[]) =>
  results
    .filter((result) => surfaces.includes(result.surface))
    .map((result) => result.speedup);

/** The figures the summaries quote, each derived from the measurements. */
export const headline = {
  endToEndSpeedup: range(speedups("e2e")),
  surfaceSpeedup: range(speedups("reader", "writer")).high,
  binaryRatio: round(binary.pandoc / binary.carta),
  memoryRatio: range(
    results.flatMap((result) =>
      result.carta_rss && result.pandoc_rss
        ? [result.pandoc_rss / result.carta_rss]
        : [],
    ),
  ),
};

/** A range as it reads in prose, collapsed to one figure when the ends round together. */
export function formatRange({ low, high }: Range): string {
  return low.toFixed(0) === high.toFixed(0)
    ? `${high.toFixed(0)}×`
    : `${low.toFixed(0)}–${high.toFixed(0)}×`;
}

/** A byte count in its largest unit, to `digits` decimals. */
export function formatBytes(value: number, digits: number): string {
  if (value >= 1_048_576) {
    return `${(value / 1_048_576).toFixed(digits)} MB`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(digits)} KB`;
  }
  return `${value} B`;
}
