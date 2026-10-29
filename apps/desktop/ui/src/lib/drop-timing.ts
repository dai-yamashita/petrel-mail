/**
 * Where the time goes between a file landing on the composer and its chip
 * being on screen, and then its copy. One line in frontend.log for each.
 *
 * A drop that feels slow has to say which part was slow: the drop reaching
 * the page, drawing the chip, or copying the bytes. Times and the size only —
 * a file's name is somebody's mail and never goes in a log.
 *
 * Every time is milliseconds after the drop event was made, which is when the
 * page had the drop in hand. `over` runs the other way: how long before it
 * the last `dragover` was heard, so a large one with the pointer released
 * mid-movement means the drop was slow to arrive at all.
 */

const clock = () => performance.now();

/** When one drop was made and first heard, before anything read it. */
export type DropMarks = { event: number; over: number | null; heard: number };

let lastOver: number | null = null;
let pending: DropMarks | null = null;

/** A file drag was heard over the composer. */
export function noteDragOver(now = clock()): void {
  lastOver = now;
}

/** A drop on the composer, before anything has read its transfer. */
export function noteDrop(eventTime: number, now = clock()): void {
  // A time on some other clock, or none, is taken as the moment it was heard.
  const event = eventTime > 0 && eventTime <= now ? eventTime : now;
  pending = { event, over: lastOver, heard: now };
  lastOver = null;
}

/** The marks of the drop being handled now. Each drop's are taken once. */
export function takeDrop(): DropMarks | null {
  const marks = pending;
  pending = null;
  return marks;
}

const span = (from: number | null, to: number | null) =>
  from === null || to === null ? '-' : String(Math.round(to - from));

/** From the drop to the frame that drew its chips. */
export function chipLine(
  marks: DropMarks,
  files: number,
  at: { handed: number; committed: number | null; painted: number },
): string {
  return [
    'drop chip',
    `files=${files}`,
    `over=${span(marks.over, marks.event)}`,
    `heard=${span(marks.event, marks.heard)}`,
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
