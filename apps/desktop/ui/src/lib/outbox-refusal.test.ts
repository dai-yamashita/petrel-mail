import { describe, expect, it } from 'vitest';
import { outboxRefusal } from './outbox-refusal';

describe('outboxRefusal', () => {
  it('says the message is going, or went, for the shell’s two refusals', () => {
    expect(outboxRefusal('that message is being sent right now')).toBe(
      'Too late: that message is being sent now.',
    );
    expect(outboxRefusal(new Error('that message is no longer in the outbox'))).toBe(
      'Too late: that message has already been sent.',
    );
  });

  it('says a message pulled back already is in Drafts', () => {
    expect(outboxRefusal('that message is already back in Drafts')).toBe(
      'That message is already back in Drafts.',
    );
  });

  it('leaves any other failure to its own wording', () => {
    expect(outboxRefusal('database is locked')).toBeNull();
    expect(outboxRefusal(undefined)).toBeNull();
  });
});
