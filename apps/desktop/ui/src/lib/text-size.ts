import { DEFAULTS } from './settings';

/** The reading text size's range, in px. One range for the two ways to change
 *  it — the slider in Settings › Appearance and the View menu's steps — so a
 *  size one of them can reach is never one the other cannot show. */
export const TEXT_SIZE_MIN = 12;
export const TEXT_SIZE_MAX = 20;

/**
 * The reading text size one step bigger or smaller, or back to the default.
 *
 * Clamped either way, so a step past the end stays at the end rather than
 * writing a size the slider has no position for. A saved value that is not a
 * number is read as the default, which is what the reader shows for it.
 */
export function stepTextSize(current: string, step: -1 | 0 | 1): string {
  if (step === 0) return DEFAULTS.readingTextSize;
  const now = Number.parseInt(current, 10);
  const from = Number.isFinite(now) ? now : Number(DEFAULTS.readingTextSize);
  return String(Math.min(TEXT_SIZE_MAX, Math.max(TEXT_SIZE_MIN, from + step)));
}
