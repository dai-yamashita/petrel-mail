/**
 * Whether selecting a row in this view means resuming a draft, not reading it.
 *
 * Drafts are unfinished mail. The composer fills the pane on the right; a
 * reading pane would show the same words in the one form they cannot edit.
 */
export function opensComposer(view: string): boolean {
  return view === 'drafts';
}

/** Leaving Drafts for another mailbox. The composer is a pane there and a
 *  floating card everywhere else, so walking out without closing it left the
 *  editor sitting on the inbox. A reply opened from the inbox is a different
 *  session: that one stays up when the mailbox changes. */
export function leavingComposerView(from: string, to: string): boolean {
  return opensComposer(from) && !opensComposer(to);
}
