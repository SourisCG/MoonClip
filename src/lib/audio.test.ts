import { describe, expect, it } from "vitest";
import {
  clampPercent,
  levelToDb,
  levelToPercent,
  meterTone,
  percentToDb,
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

describe("levelToPercent", () => {
  it("maps linear levels to the sqrt curve", () => {
    expect(levelToPercent(0)).toBe(0);
    expect(levelToPercent(0.25)).toBe(50);
    expect(levelToPercent(1)).toBe(100);
    expect(levelToPercent(2)).toBe(100);
    expect(levelToPercent(-1)).toBe(0);
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
  it("marks the OBS-like zones", () => {
    expect(meterTone(0)).toBe("ok");
    expect(meterTone(74)).toBe("ok");
    expect(meterTone(75)).toBe("warn");
    expect(meterTone(91)).toBe("warn");
    expect(meterTone(92)).toBe("hot");
    expect(meterTone(100)).toBe("hot");
  });
});
