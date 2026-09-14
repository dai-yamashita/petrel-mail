/** First paint, before the frame has measured itself. */
export const INITIAL_FRAME_HEIGHT = 180;

/** Host and reporter share this ceiling.
 *
 *  A quoted HTML thread can clear the old 20_000px cut-off, which then left
 *  the iframe at the initial height and the rest of the body unreachable.
 *  Past this the frame itself is allowed to scroll, once. */
export const FRAME_HEIGHT_CAP = 200_000;

/** The next iframe height, or null when the report is not a height.
 *
 *  Fitted posts may shrink: the scaler just told us the message occupies
 *  less space than its unscaled layout. Unfitted posts may only grow. A
 *  later layout pass that clamps getBoundingClientRect to the viewport
 *  used to shrink a finished body, which is how a search thread cut off
 *  mid-message after older cards arrived above it. */
export function nextFrameHeight(args: {
  prev: number;
  reported: unknown;
  fitted: boolean;
}): number | null {
  if (typeof args.reported !== 'number' || !Number.isFinite(args.reported)) {
    return null;
  }
  if (args.reported <= 0) return null;
  const capped = Math.min(Math.ceil(args.reported), FRAME_HEIGHT_CAP);
  if (args.fitted) return capped;
  return Math.max(args.prev, capped);
}

/** Wheel deltaY in CSS pixels for the host scroller. */
export function wheelDeltaPx(args: {
  deltaY: number;
  deltaMode: number;
  pageSize: number;
}): number {
  if (args.deltaMode === 1) return args.deltaY * 16;
  if (args.deltaMode === 2) return args.deltaY * args.pageSize;
  return args.deltaY;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return value != null && typeof value === 'object';
}
