import { describe, expect, it } from 'vitest';
import { leavingComposerView, opensComposer } from './draft-view';

describe('opensComposer', () => {
  it('is true only for the drafts mailbox', () => {
    expect(opensComposer('drafts')).toBe(true);
    expect(opensComposer('inbox')).toBe(false);
    expect(opensComposer('sent')).toBe(false);
    expect(opensComposer('outbox')).toBe(false);
    expect(opensComposer('folder:3')).toBe(false);
  });
});

describe('leavingComposerView', () => {
  it('is true only when walking out of Drafts', () => {
    expect(leavingComposerView('drafts', 'inbox')).toBe(true);
    expect(leavingComposerView('drafts', 'sent')).toBe(true);
    expect(leavingComposerView('drafts', 'folder:3')).toBe(true);
  });

  it('is false when staying in Drafts or arriving from anywhere else', () => {
    expect(leavingComposerView('drafts', 'drafts')).toBe(false);
    expect(leavingComposerView('inbox', 'drafts')).toBe(false);
    expect(leavingComposerView('inbox', 'sent')).toBe(false);
    expect(leavingComposerView('sent', 'inbox')).toBe(false);
  });
});
