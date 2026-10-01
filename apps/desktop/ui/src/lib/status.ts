import type { Status } from './api';

/**
 * The status, while it describes the account on screen; null while it is
 * still another's.
 *
 * The window switches accounts at once and hears from the status a poll
 * later. In between it held the previous account's: its count made an account
 * that held nothing look full, so the new-mail announcer took that account's
 * whole first sync as news, and its sign-in state put the wrong account's
 * name on the banner. A status that names no account is from an engine older
 * than the field, and is taken as it is.
 */
export function statusFor(status: Status | null, accountId: number | undefined): Status | null {
  if (!status || accountId === undefined) return null;
  if (status.account == null) return status;
  return status.account === accountId ? status : null;
}
