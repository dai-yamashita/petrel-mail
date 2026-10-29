import { afterEach, describe, expect, it } from 'vitest';
import { chipLine, copyLine, noteDragOver, noteDrop, takeDrop } from './drop-timing';

afterEach(() => {
  takeDrop();
});

describe('noteDrop', () => {
  it('times the drop from its event, and the last dragover before it', () => {
    noteDragOver(940);
    noteDrop(1000, 1004);
    expect(takeDrop()).toEqual({ event: 1000, over: 940, heard: 1004 });
  });

  it('takes an event time on another clock as the moment it was heard', () => {
    // An engine whose timeStamp is wall-clock time would otherwise put the
    // drop decades before it happened.
    noteDrop(1_761_700_000_000, 1004);
    expect(takeDrop()?.event).toBe(1004);
    noteDrop(0, 1004);
    expect(takeDrop()?.event).toBe(1004);
  });

  it('gives each drop its marks once', () => {
    noteDrop(1000, 1004);
    expect(takeDrop()).not.toBeNull();
    expect(takeDrop()).toBeNull();
  });

  it('does not lend one drag-over to the next drop', () => {
    noteDragOver(940);
    noteDrop(1000, 1004);
    takeDrop();
    noteDrop(2000, 2004);
    expect(takeDrop()?.over).toBeNull();
  });
});

describe('the logged lines', () => {
  const marks = { event: 1000, over: 940, heard: 1004 };

  it('say where the chip spent its time, in ms after the drop', () => {
    expect(
      chipLine(marks, 1, { handed: 1006.4, committed: 1180.6, painted: 1197 }),
    ).toBe('drop chip files=1 over=60 heard=4 handed=6 committed=181 painted=197');
  });

  it('say where the copy spent its time', () => {
    expect(
      copyLine(marks, 2_412_345, { began: 1182, read: 1194, settled: 1234 }, 'ok'),
    ).toBe('drop copy bytes=2412345 began=182 read=12 stage=40 ok');
  });

  it('leave out what was never reached', () => {
    expect(
      chipLine({ ...marks, over: null }, 1, { handed: 1006, committed: null, painted: 1197 }),
    ).toBe('drop chip files=1 over=- heard=4 handed=6 committed=- painted=197');
    expect(copyLine(marks, 10, { began: null, read: null, settled: 1300 }, 'cancelled')).toBe(
      'drop copy bytes=10 began=- read=- stage=- cancelled',
    );
  });

  it('carry numbers and nothing that names a file', () => {
    // The line is built from times and a size; there is no name to leak.
    const line = copyLine(marks, 10, { began: 1001, read: 1002, settled: 1003 }, 'failed');
    expect(line).toMatch(/^drop copy( [a-z]+=(\d+|-))+ [a-z]+$/);
  });
});
