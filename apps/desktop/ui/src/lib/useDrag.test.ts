import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useDrag } from './useDrag';

/* A press whose release never arrives.
 *
 * On the Mac a Control-click is a context click, and when WebKit shows its own
 * menu for one, the menu takes the button's release: the page never hears it.
 * The drag hook was left holding the press, so the next move carried the
 * folder with no button down, and the next click dropped it into another
 * folder on the server. Real hardware is the only thing that withholds the
 * release, so it is withheld here by hand: the press, then moves that say no
 * button is down, then the click someone makes on the folder they meant to
 * open.
 *
 * The hook runs outside React. Its window listeners read refs, not state, so
 * one call is the whole of it: the hooks are stood in for, and the effect
 * that adds the listeners runs at once. */
vi.mock('react', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react')>()),
  useState: (init: unknown) => [init, () => {}],
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

/** Whatever is under the pointer: inside the folder Receipts, over
 *  conversation -3, in neither row's edge band. */
const under = {
  closest: (selector: string) =>
    selector === '[data-folder-drop]'
      ? { dataset: { folderDrop: 'Receipts' } }
      : selector === '[data-drop-row]'
        ? { dataset: { dropRow: '-3' } }
        : null,
};

let folderDrops: [number, string][];
let tagDrops: [number, number][];
let drag: ReturnType<typeof useDrag>;

beforeEach(() => {
  vi.stubGlobal('window', new EventTarget());
  vi.stubGlobal('document', {
    body: {
      classList: { toggle: () => {} },
      setPointerCapture: () => {},
      hasPointerCapture: () => false,
      releasePointerCapture: () => {},
    },
    elementFromPoint: () => under,
  });
  folderDrops = [];
  tagDrops = [];
  drag = useDrag(
    'inbox',
    () => {},
    (tagId, threadId) => tagDrops.push([tagId, threadId]),
    (folderId, path) => folderDrops.push([folderId, path]),
    () => {},
  );
});
afterEach(() => vi.unstubAllGlobals());

/** A press on a rail row, as React hands it to the hook. */
function press(x: number, ctrlKey = false) {
  return {
    button: 0,
    ctrlKey,
    clientX: x,
    clientY: 10,
    target: { closest: () => null },
    currentTarget: {},
  } as unknown as React.PointerEvent;
}

function pointer(type: string, x: number, buttons: number) {
  window.dispatchEvent(
    Object.assign(new Event(type, { cancelable: true }), { clientX: x, clientY: 10, buttons, pointerId: 1 }),
  );
}
const move = (x: number, buttons: number) => pointer('pointermove', x, buttons);
const release = (x: number) => pointer('pointerup', x, 0);

describe('a press whose release never arrives', () => {
  it('a Control-click on a folder carries nothing to the next click', () => {
    drag.startFolder(press(10, true), 101, 'Contracts');
    // The menu kept the release: the pointer moves with no button down.
    move(40, 0);
    move(80, 0);
    // A click on Receipts, to open it.
    drag.startFolder(press(80), 7, 'Receipts');
    release(80);
    expect(folderDrops).toEqual([]);
  });

  it('a Control-click on a tag tags nothing', () => {
    drag.startTag(press(10, true), 11, 'Urgent');
    move(40, 0);
    move(80, 0);
    drag.start(press(80), -3, new Set(), 'Conversation 3');
    release(80);
    expect(tagDrops).toEqual([]);
  });

  it('moves with the button up end any press, whatever took its release', () => {
    // An ordinary press this time: a native dialog, say, took the release.
    drag.startFolder(press(10), 101, 'Contracts');
    move(40, 0);
    move(80, 0);
    drag.startFolder(press(80), 7, 'Receipts');
    release(80);
    expect(folderDrops).toEqual([]);
  });
});

describe('an ordinary drag', () => {
  it('drops even when the last move before the release already says the button is up', () => {
    // WebKit reads `buttons` when it handles a move: a drag event handled
    // just after the hardware release reports 0, and its pointerup follows.
    drag.startFolder(press(10), 101, 'Contracts');
    move(40, 1);
    move(80, 1);
    move(82, 0);
    release(82);
    expect(folderDrops).toEqual([[101, 'Receipts']]);
  });

  it('still drops what it carries where the button comes up', () => {
    drag.startFolder(press(10), 101, 'Contracts');
    move(40, 1);
    move(80, 1);
    release(80);
    expect(folderDrops).toEqual([[101, 'Receipts']]);
  });

  it('still tags the conversation a tag is carried to', () => {
    drag.startTag(press(10), 11, 'Urgent');
    move(40, 1);
    move(80, 1);
    release(80);
    expect(tagDrops).toEqual([[11, -3]]);
  });
});
