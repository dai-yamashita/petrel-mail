import { describe, expect, it } from 'vitest';
import {
  FRAME_HEIGHT_CAP,
  INITIAL_FRAME_HEIGHT,
  nextFrameHeight,
  wheelDeltaPx,
} from './frame-height';

describe('nextFrameHeight', () => {
  it('rejects zero, negatives, and non-numbers', () => {
    expect(nextFrameHeight({ prev: 180, reported: 0, fitted: false })).toBeNull();
    expect(nextFrameHeight({ prev: 180, reported: -4, fitted: false })).toBeNull();
    expect(nextFrameHeight({ prev: 180, reported: '800', fitted: false })).toBeNull();
    expect(nextFrameHeight({ prev: 180, reported: undefined, fitted: false })).toBeNull();
  });

  it('accepts a height past the old twenty-thousand cut-off', () => {
    expect(nextFrameHeight({ prev: INITIAL_FRAME_HEIGHT, reported: 25_000, fitted: false })).toBe(
      25_000,
    );
  });

  it('caps a report that would run away', () => {
    expect(
      nextFrameHeight({ prev: INITIAL_FRAME_HEIGHT, reported: FRAME_HEIGHT_CAP + 50_000, fitted: false }),
    ).toBe(FRAME_HEIGHT_CAP);
  });

  it('does not shrink an unfitted report, so a later clamp cannot cut the body', () => {
    expect(nextFrameHeight({ prev: 800, reported: 180, fitted: false })).toBe(800);
  });

  it('lets a fitted report shrink, because the scaler reduced the occupied space', () => {
    expect(nextFrameHeight({ prev: 800, reported: 400, fitted: true })).toBe(400);
  });

  it('grows from the initial height when a new message reports in', () => {
    expect(nextFrameHeight({ prev: INITIAL_FRAME_HEIGHT, reported: 640, fitted: false })).toBe(640);
  });
});

describe('wheelDeltaPx', () => {
  it('keeps pixel deltas, scales lines, and uses the page size for pages', () => {
    expect(wheelDeltaPx({ deltaY: 40, deltaMode: 0, pageSize: 600 })).toBe(40);
    expect(wheelDeltaPx({ deltaY: 3, deltaMode: 1, pageSize: 600 })).toBe(48);
    expect(wheelDeltaPx({ deltaY: 1, deltaMode: 2, pageSize: 600 })).toBe(600);
  });
});
