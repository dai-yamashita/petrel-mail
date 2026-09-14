/**
 * Which mailbox to fetch when this view is opened.
 *
 * IDLE watches the inbox. The rest wait for the five-minute sweep unless
 * we ask now. Archive is All Mail and stays off this path. Snoozed,
 * outbox and tags have no folder to SELECT.
 */
const ON_OPEN_ROLES = ['sent', 'drafts', 'spam', 'trash', 'starred'] as const;

export function syncOnOpen(view: string): string | null {
  if ((ON_OPEN_ROLES as readonly string[]).includes(view)) return view;
  if (/^folder:[1-9]\d*$/.test(view)) return view;
  return null;
}
