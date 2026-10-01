import { describe, expect, it } from 'vitest';
import { nextTool, typedIntoBody } from './toolbar';

/** The formatting toolbar is one tab stop; the arrows walk it. */
describe('nextTool', () => {
  it('moves right and left, wrapping at the ends', () => {
    expect(nextTool(0, 'ArrowRight', 11)).toBe(1);
    expect(nextTool(10, 'ArrowRight', 11)).toBe(0);
    expect(nextTool(0, 'ArrowLeft', 11)).toBe(10);
    expect(nextTool(5, 'ArrowLeft', 11)).toBe(4);
  });

  it('jumps to either end', () => {
    expect(nextTool(6, 'Home', 11)).toBe(0);
    expect(nextTool(2, 'End', 11)).toBe(10);
  });

  it('leaves every other key alone', () => {
    for (const key of ['ArrowUp', 'ArrowDown', 'Tab', 'Enter', ' ', 'b']) {
      expect(nextTool(3, key, 11), key).toBeNull();
    }
  });
});

/**
 * A letter typed on the Typeface or Size menu while it is closed. The menu's
 * own type-to-select took it: "Thanks" set the typeface to Serif at its s,
 * and only the words after it reached the message. Shift+Tab from the body
 * lands there, so this is where a sentence goes on by mistake.
 */
describe('typedIntoBody', () => {
  const key = (k: string, over = {}) => ({
    key: k,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    isComposing: false,
    ...over,
  });

  it("sends a closed menu's letters, digits, punctuation and spaces to the message", () => {
    // A space too: " Thanks" opened the menu at its space, and the open
    // menu's type-to-select then took the word.
    for (const k of ['T', 'h', 's', '7', ',', '.', '?', 'é', ' ']) {
      expect(typedIntoBody(key(k), false), JSON.stringify(k)).toBe(true);
    }
  });

  it('leaves the keys that open and walk the menu to the menu', () => {
    // Return and the arrows open it, as they open any of these menus.
    for (const k of ['Enter', 'ArrowDown', 'ArrowUp', 'Escape', 'Tab', 'Home']) {
      expect(typedIntoBody(key(k), false), k).toBe(false);
    }
  });

  it("leaves an open menu's type-to-select alone", () => {
    // The list is showing: picking an entry by typing is what it is for.
    expect(typedIntoBody(key('s'), true)).toBe(false);
  });

  it('leaves shortcuts and a word being composed alone', () => {
    expect(typedIntoBody(key('b', { metaKey: true }), false)).toBe(false);
    expect(typedIntoBody(key('b', { ctrlKey: true }), false)).toBe(false);
    expect(typedIntoBody(key('b', { altKey: true }), false)).toBe(false);
    expect(typedIntoBody(key('k', { isComposing: true }), false)).toBe(false);
  });
});
