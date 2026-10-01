import { describe, expect, it } from 'vitest';
import type { Status } from './api';
import { statusFor } from './status';

const status = (over: Partial<Status> = {}): Status => ({
  configured: true,
  demo: false,
  seeding: false,
  count: 40,
  server_total: 0,
  source: '',
  retention: '',
  data_dir: '',
  last_sync_ms: 0,
  extraction_gen: 0,
  mail_gen: 0,
  ...over,
});

/**
 * The window shows one account and polls the status every few seconds. Right
 * after a switch the status it holds is still the previous account's: its
 * count made an account that held nothing look full, and the new-mail
 * announcer took that account's whole first sync as news; its sign-in state
 * put the wrong account's name on the banner.
 */
describe('statusFor', () => {
  it('is the status while it describes the account on screen', () => {
    const s = status({ account: 2 });
    expect(statusFor(s, 2)).toBe(s);
  });

  it("is nothing while the status is still another account's", () => {
    expect(statusFor(status({ account: 1 }), 2)).toBeNull();
  });

  it('is nothing before there is a status, or an account on screen', () => {
    expect(statusFor(null, 2)).toBeNull();
    expect(statusFor(status({ account: 2 }), undefined)).toBeNull();
  });

  it('trusts a status that does not say whose it is', () => {
    // An engine from before statuses named their account.
    const s = status();
    expect(statusFor(s, 2)).toBe(s);
  });
});
