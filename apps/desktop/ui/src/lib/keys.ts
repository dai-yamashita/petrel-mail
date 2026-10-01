/**
 * Shortcut labels rendered from the running platform, never hardcoded.
 *
 * macOS concatenates modifier glyphs in a fixed order (⇧⌘O); Windows and Linux
 * spell them out and join with "+", in their own order (Ctrl+Shift+O). Same
 * binding, two vocabularies — and showing the wrong one is worse than showing
 * none, because it teaches a keystroke that does nothing. (docs 06)
 */
export const isMac =
  typeof navigator !== 'undefined' &&
  /mac/i.test(
    (navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ??
      navigator.userAgent,
  );

const MAC = {
  enter: '↵',
  account: '⌘1…9',
  send: '⌘↵',
  sendLater: '⌘⇧↵',
  bcc: '⌥⌘B',
  save: '⌘S',
  popout: '⇧⌘O',
  read: '⇧I',
  unread: '⇧U',
  extend: '⇧J ⇧K',
  find: '⌘F',
  palette: '⌘K',
  settings: '⌘,',
} as const;

const PC: Record<keyof typeof MAC, string> = {
  enter: 'Enter',
  account: 'Ctrl+1…9',
  send: 'Ctrl+Enter',
  sendLater: 'Ctrl+Shift+Enter',
  // Not Ctrl+Alt+B: on Windows that is AltGr+B, which types `{` on Polish,
  // Czech, Hungarian and other layouts. Gmail's own "add Bcc" key.
  bcc: 'Ctrl+Shift+B',
  save: 'Ctrl+S',
  popout: 'Ctrl+Shift+O',
  read: 'Shift+I',
  unread: 'Shift+U',
  extend: 'Shift+J Shift+K',
  find: 'Ctrl+F',
  palette: 'Ctrl+K',
  settings: 'Ctrl+,',
};

export type KeyName = keyof typeof MAC;

export function key(name: KeyName): string {
  return keyFor(name, isMac);
}

/** A key's label on either platform, for the one place that must not guess:
 *  a test of both. */
export function keyFor(name: KeyName, mac: boolean): string {
  return mac ? MAC[name] : PC[name];
}

/** The parts of a key press `isBccKey` reads: a DOM KeyboardEvent or React's. */
type Press = {
  key: string;
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  getModifierState(key: string): boolean;
};

/** Whether a key press asks for the Bcc field: ⌥⌘B on the Mac, as in Apple
 *  Mail, and Ctrl+Shift+B elsewhere, as in Gmail.
 *
 *  Never with AltGr held. On Windows AltGr arrives as Ctrl+Alt, so a binding
 *  with both in it takes the characters AltGr types: AltGr+B is `{` on several
 *  layouts, and the brace was swallowed while Bcc opened. On the Mac, Option
 *  turns B into another character, so the key is matched by where it is;
 *  elsewhere by the letter, as Petrel's other Ctrl shortcuts are. */
export function isBccKey(e: Press, mac: boolean = isMac): boolean {
  if (mac) return e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey && e.code === 'KeyB';
  if (e.getModifierState('AltGraph')) return false;
  return (
    e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey && e.key.toLowerCase() === 'b'
  );
}
