import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PaneResize } from './PaneResize';

/* The list/reader divider, pressed the ways WebKit's own menu answers.
 *
 * The menu takes the button's release, so the page never hears the press end:
 * the panes followed the pointer, saving their width at every move, until the
 * next click. Hardware is the only thing that withholds the release, so here
 * the moves after the press simply say no button is down. The sidebar's edge
 * goes through the same `followPress`. */

// On the Mac, where Control with the main button is a context click.
vi.mock('../lib/keys', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../lib/keys')>()),
  isMac: true,
}));

let widths: number[];
let handle: { onPointerDown: (e: React.PointerEvent) => void };

beforeEach(() => {
  vi.stubGlobal('window', new EventTarget());
  vi.stubGlobal('document', { body: { classList: { add: () => {}, remove: () => {} } } });
  widths = [];
  // No hooks in it, so the element it returns is all there is to drive.
  handle = PaneResize({ onResize: (x) => widths.push(x) }).props;
});
afterEach(() => vi.unstubAllGlobals());

function press(button: number, ctrlKey = false) {
  handle.onPointerDown({ button, ctrlKey, clientX: 430, preventDefault: () => {} } as unknown as React.PointerEvent);
}
function pointer(type: string, x: number, buttons: number) {
  window.dispatchEvent(Object.assign(new Event(type), { clientX: x, buttons }));
}

describe('the divider, after a context click', () => {
  it('does not follow the pointer after a two-finger click', () => {
    press(2);
    pointer('pointermove', 360, 0);
    pointer('pointermove', 300, 0);
    expect(widths).toEqual([]);
  });

  it('does not follow the pointer after a Control-click', () => {
    press(0, true);
    pointer('pointermove', 360, 0);
    expect(widths).toEqual([]);
  });

  it('stops at the first move with the button up, whatever took the release', () => {
    press(0);
    pointer('pointermove', 360, 0);
    pointer('pointermove', 300, 0);
    expect(widths).toEqual([]);
  });
});

describe('the divider, dragged', () => {
  it('follows the pointer while the button is down, and stops when it comes up', () => {
    press(0);
    pointer('pointermove', 360, 1);
    pointer('pointermove', 300, 1);
    pointer('pointerup', 300, 0);
    pointer('pointermove', 250, 0);
    expect(widths).toEqual([360, 300]);
  });
});
