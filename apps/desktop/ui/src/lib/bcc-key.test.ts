import { describe, expect, it } from 'vitest';
import { isBccKey, keyFor } from './keys';
import { BINDINGS, displayKeys } from './shortcuts';

/* The key that opens Bcc: ⌥⌘B on the Mac, as in Apple Mail; Ctrl+Shift+B
   elsewhere, as in Gmail. Never anything AltGr makes: on Windows AltGr
   arrives as Ctrl+Alt, and on Polish, Czech, Hungarian and other layouts
   AltGr+B types `{`, which a Ctrl+Alt+B binding swallowed. */

type Press = {
  key: string;
  code?: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
  altGraph?: boolean;
};

const press = (p: Press) => ({
  key: p.key,
  code: p.code ?? 'KeyB',
  metaKey: p.metaKey ?? false,
  ctrlKey: p.ctrlKey ?? false,
  altKey: p.altKey ?? false,
  shiftKey: p.shiftKey ?? false,
  getModifierState: (k: string) => k === 'AltGraph' && (p.altGraph ?? false),
});

describe('the Bcc key on the Mac', () => {
  it('is ⌥⌘B, which Option turns into another character', () => {
    expect(isBccKey(press({ key: '∫', metaKey: true, altKey: true }), true)).toBe(true);
  });

  it('is not ⌘B, which bolds, nor ⌘⇧B, which quotes', () => {
    expect(isBccKey(press({ key: 'b', metaKey: true }), true)).toBe(false);
    expect(isBccKey(press({ key: 'B', metaKey: true, shiftKey: true }), true)).toBe(false);
  });

  it('is labelled the Mac way', () => {
    expect(keyFor('bcc', true)).toBe('⌥⌘B');
  });
});

describe('the Bcc key on Windows and Linux', () => {
  it('is Ctrl+Shift+B', () => {
    expect(isBccKey(press({ key: 'B', ctrlKey: true, shiftKey: true }), false)).toBe(true);
  });

  it('is not Ctrl+Alt+B any more: that is AltGr+B, a brace on many layouts', () => {
    expect(isBccKey(press({ key: '{', ctrlKey: true, altKey: true, altGraph: true }), false)).toBe(
      false,
    );
    expect(isBccKey(press({ key: 'b', ctrlKey: true, altKey: true }), false)).toBe(false);
  });

  it('never fires with AltGr held, whatever else is down', () => {
    expect(
      isBccKey(press({ key: 'B', ctrlKey: true, shiftKey: true, altGraph: true }), false),
    ).toBe(false);
  });

  it('is not Ctrl+B, which bolds', () => {
    expect(isBccKey(press({ key: 'b', ctrlKey: true }), false)).toBe(false);
  });

  it('follows the layout, as the other Ctrl shortcuts do', () => {
    // Dvorak's B is where QWERTY has N; the letter is what counts.
    expect(
      isBccKey(press({ key: 'B', code: 'KeyN', ctrlKey: true, shiftKey: true }), false),
    ).toBe(true);
    expect(
      isBccKey(press({ key: 'X', code: 'KeyB', ctrlKey: true, shiftKey: true }), false),
    ).toBe(false);
  });

  it('is labelled the way Windows and Linux spell it', () => {
    expect(keyFor('bcc', false)).toBe('Ctrl+Shift+B');
  });
});

describe('Help', () => {
  it('lists the Bcc key among the keys for writing, with this platform’s label', () => {
    const bcc = BINDINGS.find((b) => b.id === 'bcc');
    expect(bcc?.group).toBe('write');
    expect(bcc?.available).toBe(true);
    expect(bcc?.label).toBe('sc-bcc');
    expect(displayKeys(bcc!)).toEqual([keyFor('bcc', bcc!.chords[0].alt === true)]);
  });
});
