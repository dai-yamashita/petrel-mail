import { describe, expect, it } from 'vitest';
import { DEFAULTS } from './settings';
import { TEXT_SIZE_MAX, TEXT_SIZE_MIN, stepTextSize } from './text-size';

describe('stepping the reading text size', () => {
  it('moves one pixel at a time', () => {
    expect(stepTextSize('15', 1)).toBe('16');
    expect(stepTextSize('15', -1)).toBe('14');
  });

  it('stops at the ends of the slider rather than past them', () => {
    // A size the slider in Settings › Appearance has no position for would
    // leave it drawn at an end while the reader showed something else.
    expect(stepTextSize(String(TEXT_SIZE_MAX), 1)).toBe(String(TEXT_SIZE_MAX));
    expect(stepTextSize(String(TEXT_SIZE_MIN), -1)).toBe(String(TEXT_SIZE_MIN));
    expect(stepTextSize('40', -1)).toBe(String(TEXT_SIZE_MAX));
  });

  it('goes back to the default, whatever it was', () => {
    expect(stepTextSize('19', 0)).toBe(DEFAULTS.readingTextSize);
    expect(stepTextSize('12', 0)).toBe(DEFAULTS.readingTextSize);
  });

  it('reads a saved value that is not a number as the default', () => {
    const next = String(Number(DEFAULTS.readingTextSize) + 1);
    expect(stepTextSize('', 1)).toBe(next);
    expect(stepTextSize('large', 1)).toBe(next);
  });
});
