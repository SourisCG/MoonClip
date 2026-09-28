import { describe, expect, it } from "vitest";
import {
  clampPercent,
  levelToDb,
  levelToPercent,
  meterTone,
  percentToDb,
  percentToLevelDb,
} from "./audio";

describe("clampPercent", () => {
  it("keeps gains inside 0–100", () => {
    expect(clampPercent(0)).toBe(0);
    expect(clampPercent(50.4)).toBe(50);
    expect(clampPercent(100)).toBe(100);
    expect(clampPercent(101)).toBe(100);
    expect(clampPercent(200)).toBe(100);
    expect(clampPercent(-5)).toBe(0);
    expect(clampPercent(Number.NaN)).toBe(100);
  });
});

describe("levelToPercent (OBS -60..0 dB scale)", () => {
  it("maps the dB scale to the bar", () => {
    expect(levelToPercent(0)).toBe(0);
    expect(levelToPercent(0.001)).toBe(0); // -60 dB floor
    expect(levelToPercent(0.1)).toBe(67); // -20 dB
    expect(levelToPercent(0.5)).toBe(90); // ~-6 dB
    expect(levelToPercent(1)).toBe(100); // 0 dB
    expect(levelToPercent(2)).toBe(100); // clamped
  });

  it("percentToLevelDb walks the same scale back", () => {
    expect(percentToLevelDb(0)).toBe(-60);
    expect(percentToLevelDb(50)).toBe(-30);
    expect(percentToLevelDb(100)).toBe(0);
  });
});

describe("dB helpers", () => {
  it("reports -inf under the noise floor and 0 dB at unity", () => {
    expect(levelToDb(0)).toBe("-inf");
    expect(levelToDb(0.0001)).toBe("-inf");
    expect(levelToDb(1)).toBe("0.0");
    expect(levelToDb(0.5)).toBe("-6.0");
  });

  it("percentToDb uses the gain scale (100 % = 0 dB)", () => {
    expect(percentToDb(100)).toBe("0.0");
    expect(percentToDb(50)).toBe("-6.0");
    expect(percentToDb(0)).toBe("-inf");
    expect(percentToDb(150)).toBe("0.0"); // clamped
  });
});

describe("meterTone", () => {
  it("matches the zone separators (-18 dB / -6 dB)", () => {
    expect(meterTone(0)).toBe("ok");
    expect(meterTone(69)).toBe("ok");
    expect(meterTone(70)).toBe("warn");
    expect(meterTone(89)).toBe("warn");
    expect(meterTone(90)).toBe("hot");
    expect(meterTone(100)).toBe("hot");
  });
});
