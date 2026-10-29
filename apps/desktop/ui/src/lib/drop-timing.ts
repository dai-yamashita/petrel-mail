import type { ComposerDrag } from './composer-drag';

/**
 * Where the time goes between a file landing on the composer and its chip
 * being on screen, and then its copy. One line in frontend.log for each.
 *
 * A drop that feels slow has to say which part was slow: the drop reaching
 * the page, drawing the chip, or copying the bytes. Times, counts and the
 * size only — a file's name is somebody's mail and never goes in a log.
 *
 * Every time is milliseconds after the drop event was made, which is when the
 * page had the drop in hand. `over` runs the other way: how long before it
 * the window last heard a `dragover` of any kind. A large one with the
 * pointer still moving as the button came up means the drop was slow to
 * arrive at all.
 */

const clock = () => performance.now();

/** What the window heard of a drag before it was dropped. */
type DragSeen = {
  last: number | null;
  before: number | null;
  count: number;
  /** How the composer read each `dragover` it saw, and the last of them. */
  kinds: Record<ComposerDrag, number>;
  kind: ComposerDrag | null;
};

/** When one drop was made and first heard, and the drag that led to it. */
export type DropMarks = {
  event: number;
  /** Whether `event` is the event's own time, not the moment it was heard. */
  stamped: boolean;
  heard: number;
  seen: DragSeen;
};

const unseen = (): DragSeen => ({
  last: null,
  before: null,
  count: 0,
  kinds: { file: 0, through: 0, field: 0 },
  kind: null,
});

let seen = unseen();
let pending: DropMarks | null = null;

/** A `dragover` anywhere in the window, whatever is being dragged. */
export function noteDragOver(now = clock()): void {
  seen.before = seen.last;
  seen.last = now;
  seen.count += 1;
}

/** How the composer read a `dragover` that crossed it. */
export function noteComposerDrag(kind: ComposerDrag): void {
  seen.kinds[kind] += 1;
  seen.kind = kind;
}

/**
 * A drop on the composer, before anything has read its transfer. What was
 * heard of the drag goes with it, so a drag that ended elsewhere still
 * counts toward the next one; nothing marks where a drag begins.
 */
export function noteDrop(eventTime: number, now = clock()): void {
  // A time on some other clock, or none, cannot be measured against.
  const stamped = eventTime > 0 && eventTime <= now;
  pending = { event: stamped ? eventTime : now, stamped, heard: now, seen };
  seen = unseen();
}

/** The marks of the drop being handled now. Each drop's are taken once. */
export function takeDrop(): DropMarks | null {
  const marks = pending;
  pending = null;
  return marks;
}

const span = (from: number | null, to: number | null) =>
  from === null || to === null ? '-' : String(Math.round(to - from));

/** From the drag, through the drop, to the frame that drew its chips. */
export function chipLine(
  marks: DropMarks,
  files: number,
  at: { handed: number; committed: number | null; painted: number },
): string {
  const { seen: drag } = marks;
  return [
    'drop chip',
    `files=${files}`,
    `over=${span(drag.last, marks.event)}`,
    `step=${span(drag.before, drag.last)}`,
    `overs=${drag.count}`,
    `file=${drag.kinds.file}`,
    `through=${drag.kinds.through}`,
    `field=${drag.kinds.field}`,
    `last=${drag.kind ?? '-'}`,
    `heard=${marks.stamped ? span(marks.event, marks.heard) : '-'}`,
    `handed=${span(marks.event, at.handed)}`,
    `committed=${span(marks.event, at.committed)}`,
    `painted=${span(marks.event, at.painted)}`,
  ].join(' ');
}

/** From the drop to one file's bytes being in staging, or to its failing. */
export function copyLine(
  marks: DropMarks,
  bytes: number,
  at: { began: number | null; read: number | null; settled: number },
  outcome: string,
): string {
  return [
    'drop copy',
    `bytes=${bytes}`,
    `began=${span(marks.event, at.began)}`,
    `read=${span(at.began, at.read)}`,
    `stage=${span(at.read, at.settled)}`,
    outcome,
  ].join(' ');
}

/**
 * Logs the chip's line once its frame has been drawn.
 *
 * React draws the chip in a microtask after the drop handler, so the next task
 * finds it in the page. The frame after that paints it, and a task queued from
 * that frame's callback runs once the paint is done.
 */
export function traceChip(
  marks: DropMarks,
  files: number,
  handed: number,
  log: (line: string) => void,
): void {
  let committed: number | null = null;
  window.setTimeout(() => {
    committed = clock();
  }, 0);
  window.requestAnimationFrame(() => {
    window.setTimeout(() => log(chipLine(marks, files, { handed, committed, painted: clock() })), 0);
  });
}

/** Times one file's copy for its line. */
export function traceCopy(marks: DropMarks, bytes: number, log: (line: string) => void) {
  let began: number | null = null;
  let read: number | null = null;
  return {
    began: () => {
      began = clock();
    },
    /** The bytes are in hand and are about to be sent. */
    read: () => {
      read = clock();
    },
    settled: (outcome: string) =>
      log(copyLine(marks, bytes, { began, read, settled: clock() }, outcome)),
  };
}
