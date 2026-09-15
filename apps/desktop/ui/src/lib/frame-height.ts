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
 *  Any positive report is taken, shorter ones included: the frame measures
 *  its own document, and a short message has to come down from the first
 *  paint's height. A runaway report is capped, not refused. */
export function nextFrameHeight(reported: unknown): number | null {
  if (typeof reported !== 'number' || !Number.isFinite(reported)) return null;
  if (reported <= 0) return null;
  return Math.min(Math.ceil(reported), FRAME_HEIGHT_CAP);
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return value != null && typeof value === 'object';
}
