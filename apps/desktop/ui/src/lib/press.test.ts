import { describe, expect, it } from 'vitest';
import { pressEnded, startsDrag } from './press';

/* Which presses may begin a drag, a resize or a move, on both platforms. The
   hooks that use the rule are driven through a lost release in their own
   tests (useDrag, PaneResize, drag-window). */

describe('startsDrag', () => {
  it('takes the main button', () => {
    expect(startsDrag({ button: 0, ctrlKey: false }, true)).toBe(true);
    expect(startsDrag({ button: 0, ctrlKey: false }, false)).toBe(true);
  });

  it('never takes a two-finger click, a middle click or a back button', () => {
    for (const button of [1, 2, 3, 4]) {
      expect(startsDrag({ button, ctrlKey: false }, true), String(button)).toBe(false);
      expect(startsDrag({ button, ctrlKey: false }, false), String(button)).toBe(false);
    }
  });

  it('refuses a Control-click on the Mac, where it is the context click', () => {
    expect(startsDrag({ button: 0, ctrlKey: true }, true)).toBe(false);
  });

  it('leaves Control with the main button alone elsewhere, where it is not', () => {
    expect(startsDrag({ button: 0, ctrlKey: true }, false)).toBe(true);
  });
});

describe('pressEnded', () => {
  it('reads a move with the main button up as the end of the press', () => {
    expect(pressEnded({ buttons: 0 })).toBe(true);
    // Only the right button down: the main one is still up.
    expect(pressEnded({ buttons: 2 })).toBe(true);
  });

  it('reads a move with the main button down as the press going on', () => {
    expect(pressEnded({ buttons: 1 })).toBe(false);
    expect(pressEnded({ buttons: 3 })).toBe(false);
  });
});
