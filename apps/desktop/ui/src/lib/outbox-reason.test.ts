import { afterEach, describe, expect, it } from 'vitest';
import { heldReason } from './outbox-reason';
import { setLocale } from './strings';
import sendRs from '../../../src-tauri/src/send.rs?raw';

afterEach(() => setLocale('en'));

describe('heldReason', () => {
  /* Review r2, R2-4. The shell refuses these two before any server is asked,
     and words them in English. Shown inside "Rejected: …", German read
     "Abgewiesen: not an address: Dana Wu." */
  it('names the entry that is no address, in the person’s language', () => {
    setLocale('de');
    expect(heldReason('not an address: Dana Wu, "Wu')).toBe(
      'Es wurde nichts gesendet. Keine Adresse: Dana Wu, "Wu.',
    );
    setLocale('ja');
    expect(heldReason('not an address: Dana Wu')).toBe(
      '何も送信されていません。アドレスではありません: Dana Wu。',
    );
    setLocale('en');
    expect(heldReason('not an address: Dana Wu')).toBe('Nothing was sent. Not an address: Dana Wu.');
  });

  it('says a message with nobody to send to has no recipient', () => {
    setLocale('fr');
    expect(heldReason('a message needs at least one recipient')).toBe(
      'Rien n’a été envoyé. Le message n’a aucun destinataire.',
    );
  });

  it('folds the whitespace a pasted name can carry, as the row always has', () => {
    expect(heldReason('not an address: Dana\tWu,\n bob')).toBe('Nothing was sent. Not an address: Dana Wu, bob.');
  });

  it('leaves a server’s refusal in the server’s words', () => {
    expect(heldReason('550 5.1.1 <address> no such user')).toBeNull();
    expect(heldReason('auth: 535 authentication failed')).toBeNull();
    expect(heldReason(null)).toBeNull();
  });

  it('matches the words the shell writes', () => {
    // Read from the shell's source, so a change to either side shows here
    // rather than as English on an Outbox row.
    expect(sendRs).toContain('const NOT_AN_ADDRESS: &str = "not an address";');
    expect(sendRs).toContain('const NO_RECIPIENT: &str = "a message needs at least one recipient";');
  });
});
