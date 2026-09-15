import { describe, expect, it } from 'vitest';
import { syncOnOpen } from './mailbox-sync';

describe('syncOnOpen', () => {
  it('asks the server for sweep folders and user folders', () => {
    expect(syncOnOpen('sent')).toBe('sent');
    expect(syncOnOpen('drafts')).toBe('drafts');
    expect(syncOnOpen('spam')).toBe('spam');
    expect(syncOnOpen('trash')).toBe('trash');
    expect(syncOnOpen('starred')).toBe('starred');
    expect(syncOnOpen('folder:3')).toBe('folder:3');
  });

  it('leaves inbox, archive, and local-only views alone', () => {
    expect(syncOnOpen('inbox')).toBeNull();
    expect(syncOnOpen('archive')).toBeNull();
    expect(syncOnOpen('outbox')).toBeNull();
    expect(syncOnOpen('snoozed')).toBeNull();
    expect(syncOnOpen('tag:urgent')).toBeNull();
    expect(syncOnOpen('folder:0')).toBeNull();
    expect(syncOnOpen('folder:nope')).toBeNull();
  });
});
