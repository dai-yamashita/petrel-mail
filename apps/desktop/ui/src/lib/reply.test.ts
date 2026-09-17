import { describe, expect, it } from 'vitest';
import { replyHeaders, replyTargets } from './reply';
import type { ThreadMessage } from './api';

const message = (over: Partial<ThreadMessage> = {}): ThreadMessage =>
  ({
    id: 1,
    from_display: 'Sam',
    from_addr: 'sam@example.com',
    subject: 'Hi',
    snippet: '',
    date_ms: 0,
    unread: false,
    recipients: [],
    recipient_addrs: ['you@example.com', 'dana@example.com'],
    attachments: [],
    ...over,
  }) as ThreadMessage;

describe('replyHeaders', () => {
  it('names the message answered and appends it to its own chain', () => {
    expect(
      replyHeaders({ msgid: 'c@example.com', references: ['a@example.com', 'b@example.com'] }),
    ).toEqual({
      inReplyTo: 'c@example.com',
      references: ['a@example.com', 'b@example.com', 'c@example.com'],
    });
  });

  it('starts a chain for a message that had none', () => {
    expect(replyHeaders({ msgid: 'a@example.com', references: [] })).toEqual({
      inReplyTo: 'a@example.com',
      references: ['a@example.com'],
    });
  });

  it('sends nothing for a message with no id, rather than an empty header', () => {
    // A bare "<>" in In-Reply-To is worse than no header: some clients file
    // it under a conversation called nothing.
    expect(replyHeaders({ msgid: null, references: [] })).toEqual({
      inReplyTo: null,
      references: [],
    });
    expect(replyHeaders({ msgid: '  ', references: ['a@example.com'] })).toEqual({
      inReplyTo: null,
      references: ['a@example.com'],
    });
  });

  it('never lists the same id twice', () => {
    expect(
      replyHeaders({ msgid: 'a@example.com', references: ['a@example.com'] }).references,
    ).toEqual(['a@example.com']);
  });
});

describe('reply', () => {
  it('goes to the sender', () => {
    expect(replyTargets(message(), 'you@example.com', false)).toEqual({
      to: ['sam@example.com'],
      cc: [],
    });
  });

  it('copies nobody', () => {
    expect(replyTargets(message(), 'you@example.com', false).cc).toEqual([]);
  });
});

describe('reply all', () => {
  it('copies everyone else on the thread', () => {
    const { to, cc } = replyTargets(message(), 'you@example.com', true);
    expect(to).toEqual(['sam@example.com']);
    expect(cc).toEqual(['dana@example.com']);
  });

  it('never copies you on your own reply', () => {
    // Nothing looks more broken, and on a long thread it compounds.
    const { to, cc } = replyTargets(message(), 'you@example.com', true);
    expect([...to, ...cc]).not.toContain('you@example.com');
  });

  it('ignores case when deciding that an address is yours', () => {
    const m = message({ recipient_addrs: ['You@Example.com', 'dana@example.com'] });
    expect(replyTargets(m, 'you@example.com', true).cc).toEqual(['dana@example.com']);
  });

  it('does not list the sender twice when they are also a recipient', () => {
    const m = message({ recipient_addrs: ['sam@example.com', 'dana@example.com'] });
    const { to, cc } = replyTargets(m, 'you@example.com', true);
    expect(to).toEqual(['sam@example.com']);
    expect(cc).toEqual(['dana@example.com']);
  });

  it('degrades to a plain reply when there is nobody else', () => {
    const m = message({ recipient_addrs: ['you@example.com'] });
    expect(replyTargets(m, 'you@example.com', true)).toEqual({
      to: ['sam@example.com'],
      cc: [],
    });
  });
});

describe('replying to a message you sent', () => {
  const mine = message({
    from_addr: 'you@example.com',
    from_display: 'You',
    recipient_addrs: ['sam@example.com', 'dana@example.com'],
  });

  it('writes to the people you wrote to, not to yourself', () => {
    // Following up on your own message is the ordinary reason to reply to
    // it. Addressing the sender gave an empty To, because the sender is you.
    expect(replyTargets(mine, 'you@example.com', false)).toEqual({
      to: ['sam@example.com', 'dana@example.com'],
      cc: [],
    });
  });

  it('never leaves the To empty on a reply-all either', () => {
    const { to, cc } = replyTargets(mine, 'you@example.com', true);
    expect(to).toEqual(['sam@example.com', 'dana@example.com']);
    expect(cc).toEqual([]);
  });

  it('still replies to the sender when the sender is somebody else', () => {
    expect(replyTargets(message(), 'you@example.com', false).to).toEqual(['sam@example.com']);
  });
});

/* RFC 5322 §3.6.2: Reply-To names where the author asks replies to go.
   Ignoring it is not a cosmetic miss. A GitHub notification arrives From a
   no-reply address and names a `reply+…@reply.github.com` in Reply-To; a reply
   sent to the From address is accepted by nobody and posts nothing. The message
   leaves, and that is the last anyone hears of it. */
describe('reply honours where the author asked to be answered', () => {
  const asked = ['reply+abc@reply.github.example'];

  it('writes to the reply-to instead of the sender', () => {
    expect(replyTargets(message(), 'you@example.com', false, asked)).toEqual({
      to: ['reply+abc@reply.github.example'],
      cc: [],
    });
  });

  it('still writes to the sender when nothing was asked', () => {
    expect(replyTargets(message(), 'you@example.com', false, []).to).toEqual(['sam@example.com']);
  });

  it('keeps the author in the loop on a reply-all', () => {
    // They are no longer the recipient, but they still wrote it.
    const { to, cc } = replyTargets(message(), 'you@example.com', true, asked);
    expect(to).toEqual(['reply+abc@reply.github.example']);
    expect(cc).toContain('sam@example.com');
  });

  it('writes to nobody twice', () => {
    const { to, cc } = replyTargets(message(), 'you@example.com', true, ['sam@example.com']);
    expect(to).toEqual(['sam@example.com']);
    expect(cc).not.toContain('sam@example.com');
  });

  it('never writes to you, whoever asked', () => {
    const { to, cc } = replyTargets(message(), 'you@example.com', true, ['you@example.com']);
    expect(to).not.toContain('you@example.com');
    expect(cc).not.toContain('you@example.com');
  });

  it('takes every address a list names', () => {
    const both = ['one@example.com', 'two@example.com'];
    expect(replyTargets(message(), 'you@example.com', false, both).to).toEqual(both);
  });

  it('leaves a follow-up to your own message alone', () => {
    // Answering yourself means writing to the people you wrote to, and a
    // reply-to on your own message does not change who those are.
    const mine = { ...message(), from_addr: 'you@example.com' };
    const { to } = replyTargets(mine, 'you@example.com', false, asked);
    expect(to).not.toContain('reply+abc@reply.github.example');
  });
});
