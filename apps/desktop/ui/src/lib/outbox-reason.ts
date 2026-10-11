import { t } from './strings';

/** How the shell words its own two refusals (`NOT_AN_ADDRESS` and
 *  `NO_RECIPIENT` in send.rs). The entries follow the first. */
const NOT_AN_ADDRESS = 'not an address:';
const NO_RECIPIENT = 'a message needs at least one recipient';

/**
 * Why the Outbox holds a message for good, when the reason is Petrel's own,
 * in the person's language.
 *
 * Two refusals are made before any server is asked (send.rs): an entry that
 * is not an address, and a message with nobody to send to. The shell words
 * them in English, and shown inside "Rejected: …" German read "Abgewiesen:
 * not an address: Dana Wu." The entries stay as they were typed, so the one
 * to fix is still named. Null for anything else: a server's refusal keeps
 * its own words, which are what the server said, code and all.
 */
export function heldReason(error: string | null): string | null {
  // Folded as the row folds every reason: a name pasted from two cells
  // carries a tab.
  const text = (error ?? '').replace(/\s+/g, ' ').trim();
  if (text.startsWith(NOT_AN_ADDRESS)) {
    return t('outbox-not-an-address', { entries: text.slice(NOT_AN_ADDRESS.length).trim() });
  }
  if (text === NO_RECIPIENT) return t('outbox-no-recipient');
  return null;
}
