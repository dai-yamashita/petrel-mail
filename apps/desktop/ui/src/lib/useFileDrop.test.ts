import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { dropCover } from './useFileDrop';

describe('dropCover', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('comes off on the drop, not when the clock runs out', () => {
    // The editor stops the drop it takes. Waiting for silence instead kept
    // the cover over the chip the drop had just added.
    const seen: boolean[] = [];
    const cover = dropCover((over) => seen.push(over));
    cover.dragOver();
    cover.drop();
    expect(seen).toEqual([true, false]);
    vi.advanceTimersByTime(1000);
    expect(seen).toEqual([true, false]);
  });

  it('comes off by itself when the drag leaves without a drop', () => {
    const seen: boolean[] = [];
    const cover = dropCover((over) => seen.push(over));
    cover.dragOver();
    vi.advanceTimersByTime(159);
    expect(seen).toEqual([true]);
    vi.advanceTimersByTime(1);
    expect(seen).toEqual([true, false]);
  });

  it('stays up while the drag keeps reporting in', () => {
    const seen: boolean[] = [];
    const cover = dropCover((over) => seen.push(over));
    for (let i = 0; i < 5; i++) {
      cover.dragOver();
      vi.advanceTimersByTime(100);
    }
    expect(seen.every(Boolean)).toBe(true);
  });
});
