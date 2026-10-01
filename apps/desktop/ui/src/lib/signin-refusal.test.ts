import { describe, expect, it } from 'vitest';
import { folderPending, signinRefusal } from './signin-refusal';

describe('signinRefusal', () => {
  it('puts the shell refusal in the person’s words', () => {
    expect(signinRefusal('sign in to this account again first')).toMatch(/Sign in again first/);
    expect(signinRefusal(new Error('sign in to this account again first'))).toMatch(/Sign in again first/);
  });

  it('leaves every other failure to its own wording', () => {
    expect(signinRefusal('imap: no response: NONEXISTENT')).toBeNull();
    expect(signinRefusal(undefined)).toBeNull();
  });
});

describe('folderPending', () => {
  it('says a folder made while signed out is here, and goes once signed in', () => {
    const said = folderPending('Projects', 'sign in to this account again first');
    expect(said).toBe('Made “Projects” here. It goes to the server once you sign in again.');
  });

  it('gives any other failure the server’s own words', () => {
    expect(folderPending('Projects', 'imap: no response: NO [CANNOT] bad name')).toBe(
      'Couldn’t create “Projects” on the server yet: imap: no response: NO [CANNOT] bad name. It is kept here, and the next sync tries again.',
    );
  });
});
