import { describe, expect, it } from 'vitest';
import { FRAME_HEIGHT_CAP, INITIAL_FRAME_HEIGHT, nextFrameHeight } from './frame-height';

describe('nextFrameHeight', () => {
  it('rejects zero, negatives, and non-numbers', () => {
    expect(nextFrameHeight(0)).toBeNull();
    expect(nextFrameHeight(-4)).toBeNull();
    expect(nextFrameHeight('800')).toBeNull();
    expect(nextFrameHeight(undefined)).toBeNull();
    expect(nextFrameHeight(Number.NaN)).toBeNull();
  });

  it('accepts a height past the old twenty-thousand cut-off', () => {
    expect(nextFrameHeight(25_000)).toBe(25_000);
  });

  it('caps a report that would run away', () => {
    expect(nextFrameHeight(FRAME_HEIGHT_CAP + 50_000)).toBe(FRAME_HEIGHT_CAP);
  });

  it('lets a short message come down from the first paint', () => {
    // A one-line reply measures well under the starting height; keeping
    // the frame at 180 drew a blank band under every short message.
    expect(nextFrameHeight(52)).toBe(52);
    expect(nextFrameHeight(145)).toBeLessThan(INITIAL_FRAME_HEIGHT);
  });

  it('rounds up, so a fractional layout never clips a line', () => {
    expect(nextFrameHeight(640.2)).toBe(641);
  });
});
