/**
 * Audio math shared by the mixer, the meters and the tests (vitest).
 * All UI gains are 0–100 (100 = unity, the user's choice); levels are linear
 * amplitudes (1.0 = full scale) as reported by OBS `InputVolumeMeters`.
 */

/** Clamp a gain percent to the 0–100 supported range. */
export function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 100;
  return Math.max(0, Math.min(100, Math.round(value)));
}

/** Linear amplitude -> 0–100 meter percent (sqrt curve, gentler than linear). */
export function levelToPercent(level: number): number {
  if (!Number.isFinite(level) || level <= 0) return 0;
  return Math.min(100, Math.round(Math.sqrt(Math.min(level, 1)) * 100));
}

/** Linear amplitude -> dB string ("-inf" below the noise floor). */
export function levelToDb(level: number): string {
  if (!Number.isFinite(level) || level <= 0.0005) return "-inf";
  return `${(20 * Math.log10(level)).toFixed(1)}`;
}

/** Gain percent -> dB string (100% = 0 dB, 50% ≈ -6.0 dB, 0% = -inf). */
export function percentToDb(percent: number): string {
  const pct = clampPercent(percent);
  if (pct <= 0) return "-inf";
  return `${(20 * Math.log10(pct / 100)).toFixed(1)}`;
}

export type MeterTone = "ok" | "warn" | "hot";

/** Meter color zones: green until -6 dB-ish, gold near the top, red clipping. */
export function meterTone(percent: number): MeterTone {
  if (percent >= 92) return "hot";
  if (percent >= 75) return "warn";
  return "ok";
}
