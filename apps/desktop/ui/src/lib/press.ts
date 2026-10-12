import type React from 'react';
import { isMac } from './keys';

/**
 * Presses that begin something only their release can end: a drag, a
 * divider's resize, the composer's move.
 *
 * Each of them waits for the button to come up, and on the Mac the page does
 * not always hear it. A Control-click is a context click there, though its
 * press still says it is the main button. When WebKit answers a context click
 * with its own menu, the menu takes the release and the page is never told
 * (WebPageProxy::showContextMenu discards it). Whatever the press began was
 * left running: the next move carried a sidebar folder with no button held,
 * and the next click dropped it into another folder on the server. A divider
 * followed the pointer, saving its width as it went.
 *
 * So a context click begins nothing, and a move that arrives with the button
 * up ends what a press began, whatever became of its release.
 */

/** Whether a press may begin a drag, a resize or a move: the main button, and
 *  not the Control-click that is a context click on the Mac. */
export function startsDrag(e: { button: number; ctrlKey: boolean }, mac: boolean = isMac): boolean {
  return e.button === 0 && !(mac && e.ctrlKey);
}

/** Whether the press is over though no release arrived: the pointer moved
 *  with the main button up. */
export function pressEnded(e: { buttons: number }): boolean {
  return (e.buttons & 1) === 0;
}

/**
 * Follows a divider's press until the button comes up, reporting the
 * pointer's x on every move.
 *
 * The listeners go on the window, not the handle: a fast drag outruns a 6px
 * target, and a pointer lost mid-drag would leave the panes at whatever width
 * the last event happened to land on.
 */
export function followPress(e: React.PointerEvent, onMove: (x: number) => void, mac: boolean = isMac) {
  if (!startsDrag(e, mac)) return;
  e.preventDefault();
  const move = (ev: PointerEvent) => {
    if (pressEnded(ev)) end();
    else onMove(ev.clientX);
  };
  const end = () => {
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', end);
    document.body.classList.remove('resizing');
  };
  document.body.classList.add('resizing');
  window.addEventListener('pointermove', move);
  window.addEventListener('pointerup', end);
}
