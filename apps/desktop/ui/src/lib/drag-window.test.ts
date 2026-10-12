import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useDragWindow } from './drag-window';

/* The floating composer's title bar, after a Control-click.
 *
 * WebKit's menu took the release, and the composer followed the pointer,
 * holding it, until the next click: nothing else in the window got hover.
 * The hook runs outside React here, once; where the composer would move to
 * is whatever it asks `setOffset` for. */
const moves = vi.hoisted(() => [] as unknown[]);
vi.mock('react', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react')>()),
  useState: (init: unknown) => [init, (next: unknown) => moves.push(next)],
  useRef: (init: unknown) => ({ current: init }),
  useCallback: (fn: unknown) => fn,
  useEffect: (effect: () => void) => {
    effect();
  },
}));
// On the Mac, where Control with the main button is a context click.
vi.mock('./keys', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./keys')>()),
  isMac: true,
}));

let handle: ReturnType<typeof useDragWindow>['handleProps'];
let held: boolean;
const head = {
  setPointerCapture: () => {
    held = true;
  },
  hasPointerCapture: () => held,
  releasePointerCapture: () => {
    held = false;
  },
};

beforeEach(() => {
  vi.stubGlobal('window', Object.assign(new EventTarget(), { innerWidth: 1280, innerHeight: 800 }));
  moves.length = 0;
  held = false;
  handle = useDragWindow().handleProps;
});
afterEach(() => vi.unstubAllGlobals());

const event = (fields: object) =>
  ({
    pointerId: 1,
    target: { closest: () => null },
    currentTarget: head,
    preventDefault: () => {},
    ...fields,
  }) as unknown as React.PointerEvent;

describe("the composer's title bar", () => {
  it('is not grabbed by a Control-click', () => {
    handle.onPointerDown(event({ button: 0, ctrlKey: true, clientX: 500, clientY: 100 }));
    handle.onPointerMove(event({ buttons: 0, clientX: 300, clientY: 300 }));
    expect(moves).toEqual([]);
    expect(held).toBe(false);
  });

  it('lets go at the first move with the button up, whatever took the release', () => {
    handle.onPointerDown(event({ button: 0, ctrlKey: false, clientX: 500, clientY: 100 }));
    expect(held).toBe(true);
    handle.onPointerMove(event({ buttons: 0, clientX: 300, clientY: 300 }));
    expect(moves).toEqual([]);
    expect(held).toBe(false);
  });

  it('still moves the composer while the button is down', () => {
    handle.onPointerDown(event({ button: 0, ctrlKey: false, clientX: 500, clientY: 100 }));
    handle.onPointerMove(event({ buttons: 1, clientX: 300, clientY: 40 }));
    expect(moves).toEqual([{ x: -200, y: -60 }]);
  });
});
