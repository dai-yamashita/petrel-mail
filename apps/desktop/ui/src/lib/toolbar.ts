/**
 * Where an arrow key moves focus along a toolbar of `count` controls.
 *
 * The composer's formatting toolbar is one tab stop, as a toolbar should be:
 * thirteen stops between Subject and the body made Tab from Subject land on
 * the font menu, and the next letters typed went to the mailbox behind it.
 * Inside it, the arrows walk the controls and wrap; Home and End go to the
 * ends. Null for any other key, which the control keeps for itself.
 */
export function nextTool(current: number, key: string, count: number): number | null {
  switch (key) {
    case 'ArrowRight':
      return (current + 1) % count;
    case 'ArrowLeft':
      return (current - 1 + count) % count;
    case 'Home':
      return 0;
    case 'End':
      return count - 1;
    default:
      return null;
  }
}

/**
 * Whether a key pressed on one of the toolbar's closed menus — Typeface, Size
 * — is writing, and belongs in the message rather than the menu.
 *
 * A closed menu picks an entry by typing, as a select does, and Shift+Tab
 * from the body lands on the first of them: "Thanks" typed there set the
 * typeface to Serif at its s, and only the words after it reached the
 * message. A letter, digit, mark or space goes to the message instead — a
 * space too, which opened the menu, and the open menu's type-to-select then
 * took the word after it. Return and the arrows still open the menu and walk
 * it, and once it is open, typing to pick an entry is what it is for.
 */
export function typedIntoBody(
  e: { key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; isComposing: boolean },
  open: boolean,
): boolean {
  if (open || e.isComposing || e.metaKey || e.ctrlKey || e.altKey) return false;
  return [...e.key].length === 1;
}
