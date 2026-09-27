/**
 * Which mailbox to fetch when this view is opened.
 *
 * IDLE watches the inbox. The rest wait for the five-minute sweep unless
 * we ask now. Archive included: on a classic server it is a folder like the
 * others, and on Gmail, where it is All Mail, the engine has nothing to fetch
 * and says so without asking the server. Snoozed, outbox and tags have no
 * folder to SELECT.
 */
const ON_OPEN_ROLES = ['sent', 'drafts', 'spam', 'trash', 'starred', 'archive'] as const;

export function syncOnOpen(view: string): string | null {
  if ((ON_OPEN_ROLES as readonly string[]).includes(view)) return view;
  if (/^folder:[1-9]\d*$/.test(view)) return view;
  return null;
}
