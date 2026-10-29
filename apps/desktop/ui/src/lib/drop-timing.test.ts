import { afterEach, describe, expect, it } from 'vitest';
import {
  chipLine,
  copyLine,
  noteComposerDrag,
  noteDragOver,
  noteDrop,
  takeDrop,
  type DropMarks,
} from './drop-timing';

afterEach(() => {
  noteDrop(1, 1);
  takeDrop();
});

describe('noteDrop', () => {
  it('keeps the drag that led to the drop, and the event time', () => {
    noteDragOver(900);
    noteComposerDrag('file');
    noteDragOver(940);
    noteComposerDrag('through');
    noteDrop(1000, 1004);
    expect(takeDrop()).toEqual({
      event: 1000,
      stamped: true,
      heard: 1004,
      seen: {
        last: 940,
        before: 900,
        count: 2,
        kinds: { file: 1, through: 1, field: 0 },
        kind: 'through',
      },
    });
  });

  it('does not measure against an event time on another clock', () => {
    // An engine whose timeStamp is wall-clock time would otherwise put the
    // drop decades before it happened.
    noteDrop(1_761_700_000_000, 1004);
    expect(takeDrop()).toMatchObject({ event: 1004, stamped: false });
    noteDrop(0, 1004);
    expect(takeDrop()).toMatchObject({ event: 1004, stamped: false });
  });

  it('gives each drop its marks once', () => {
    noteDrop(1000, 1004);
    expect(takeDrop()).not.toBeNull();
    expect(takeDrop()).toBeNull();
  });

  it('does not lend one drag to the next drop', () => {
    noteDragOver(940);
    noteComposerDrag('file');
    noteDrop(1000, 1004);
    takeDrop();
    noteDrop(2000, 2004);
    expect(takeDrop()?.seen).toMatchObject({ last: null, count: 0, kind: null });
  });
});

describe('the logged lines', () => {
  const marks: DropMarks = {
    event: 1000,
    stamped: true,
    heard: 1004,
    seen: {
      last: 940,
      before: 924,
      count: 31,
      kinds: { file: 20, through: 9, field: 0 },
      kind: 'file',
    },
  };

  it('say how the drag ended and where the chip spent its time', () => {
    expect(chipLine(marks, 1, { handed: 1006.4, committed: 1180.6, painted: 1197 })).toBe(
      'drop chip files=1 over=60 step=16 overs=31 file=20 through=9 field=0 last=file' +
        ' heard=4 handed=6 committed=181 painted=197',
    );
  });

  it('say where the copy spent its time', () => {
    expect(copyLine(marks, 2_412_345, { began: 1182, read: 1194, settled: 1234 }, 'ok')).toBe(
      'drop copy bytes=2412345 began=182 read=12 stage=40 ok',
    );
  });

  it('leave out what was never reached or cannot be measured', () => {
    const unheard: DropMarks = {
      ...marks,
      stamped: false,
      seen: { last: null, before: null, count: 0, kinds: { file: 0, through: 0, field: 0 }, kind: null },
    };
    expect(chipLine(unheard, 1, { handed: 1006, committed: null, painted: 1197 })).toBe(
      'drop chip files=1 over=- step=- overs=0 file=0 through=0 field=0 last=-' +
        ' heard=- handed=6 committed=- painted=197',
    );
    expect(copyLine(marks, 10, { began: null, read: null, settled: 1300 }, 'cancelled')).toBe(
      'drop copy bytes=10 began=- read=- stage=- cancelled',
    );
  });

  it('carry numbers and words of their own, and nothing that names a file', () => {
    const line = chipLine(marks, 1, { handed: 1001, committed: 1002, painted: 1003 });
    expect(line).toMatch(/^drop chip( [a-z]+=(\d+|-|file|through|field))+$/);
  });
});
