import { t } from './strings';

/**
 * The shell's refusal to pull back or discard a message it can no longer
 * stop, in words that say what happened.
 *
 * It answers in English, in one of three forms (commands/outbox.rs, and the
 * store's `pull_back`): the message is on the wire; it is gone, sent; or it
 * was already pulled back into Drafts, as a new draft because it had been
 * deleted while it waited. Put inside "Could not open that draft: …", the
 * first read as a failure of the app and the second said nothing about the
 * message having gone. Anything else is a real failure and keeps its own
 * wording.
 */
export function outboxRefusal(error: unknown): string | null {
  const text = String(error);
  if (text.includes('that message is being sent right now')) return t('outbox-too-late-sending');
  if (text.includes('that message is no longer in the outbox')) return t('outbox-too-late-sent');
  if (text.includes('that message is already back in Drafts')) return t('outbox-already-back');
  return null;
}
