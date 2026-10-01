import { describe, expect, it } from 'vitest';
import { DEFAULTS, type Settings } from './settings';
import {
  notifiable,
  shouldNotify,
  arrivalsSince,
  announceStep,
  desktopNotice,
  elsewhereNotices,
  type Announced,
} from './notify';
import type { Thread } from './api';

/**
 * The notification rules decide when to interrupt someone, which is the kind of
 * thing that has to be right rather than nearly right: a paused app that still
 * buzzes has broken its only promise.
 */

const settings = (over: Partial<Settings> = {}): Settings => ({ ...DEFAULTS, ...over });

let n = 0;
const thread = (over: Partial<Thread> = {}): Thread =>
  ({
    thread_id: -++n,
    id: n,
    from_display: 'Sam',
    from_addr: 'sam@example.com',
    subject: 'Subject',
    snippet: '',
    date_ms: 0,
    message_count: 1,
    participants: 'Sam',
    unread: true,
    starred: false,
    has_attachments: false,
    tags: [],
    attachment_name: '',
    ...over,
  }) as Thread;

const NOW = 1_000_000;

describe('pause', () => {
  it('silences everything while it is running', () => {
    const s = settings({ notifyPausedUntil: String(NOW + 60_000) });
    expect(shouldNotify(s, NOW)).toBe(false);
    expect(notifiable(s, [thread()], NOW)).toHaveLength(0);
  });

  it('lapses on its own rather than needing to be switched back', () => {
    // Stored as an instant precisely so it cannot be left on by accident.
    const s = settings({ notifyPausedUntil: String(NOW - 1) });
    expect(shouldNotify(s, NOW)).toBe(true);
  });

  it('treats an absent or unparseable value as not paused', () => {
    expect(shouldNotify(settings({ notifyPausedUntil: '0' }), NOW)).toBe(true);
    expect(shouldNotify(settings({ notifyPausedUntil: '' }), NOW)).toBe(true);
    expect(shouldNotify(settings({ notifyPausedUntil: 'nonsense' }), NOW)).toBe(true);
  });
});

describe('level', () => {
  it('announces every unread arrival on "all"', () => {
    const s = settings({ notifyLevel: 'all' });
    expect(notifiable(s, [thread(), thread()], NOW)).toHaveLength(2);
  });

  it('announces only starred arrivals on "priority"', () => {
    const s = settings({ notifyLevel: 'priority' });
    const got = notifiable(s, [thread(), thread({ starred: true })], NOW);
    expect(got).toHaveLength(1);
    expect(got[0].starred).toBe(true);
  });

  it('announces nothing on "none", pause or no pause', () => {
    const s = settings({ notifyLevel: 'none' });
    expect(notifiable(s, [thread({ starred: true })], NOW)).toHaveLength(0);
    expect(shouldNotify(s, NOW)).toBe(false);
  });
});

describe('what counts as an arrival', () => {
  it('ignores mail that arrived already read', () => {
    // Sent from another device, or already seen elsewhere. Announcing it is
    // announcing something the user has demonstrably dealt with.
    const s = settings();
    expect(notifiable(s, [thread({ unread: false })], NOW)).toHaveLength(0);
  });

  it('says nothing when nothing arrived', () => {
    expect(notifiable(settings(), [], NOW)).toHaveLength(0);
  });
});

describe('arrivalsSince', () => {
  const row = (id: number, date_ms: number) =>
    ({ id, thread_id: id, date_ms, unread: true }) as unknown as Parameters<typeof arrivalsSince>[0][number];

  it('announces only what is newer than anything announced before', () => {
    const announced = new Set([1, 2]);
    const items = [row(3, 5000), row(1, 4000), row(2, 3000), row(9, 100)];
    expect(arrivalsSince(items, announced, 4000).map((m: { id: number }) => m.id)).toEqual([3]);
  });

  it('is quiet for a page of older conversations', () => {
    const announced = new Set([1, 2]);
    const older = [row(1, 4000), row(2, 3000), row(7, 2000), row(8, 1000)];
    expect(arrivalsSince(older, announced, 4000)).toEqual([]);
  });
});


describe('announceStep', () => {
  const unread13 = Array.from({ length: 13 }, (_, i) => thread({ id: 500 + i, date_ms: 10_000 - i }));
  type List = Parameters<typeof announceStep>[1];
  const loaded = (items: Thread[], over: Partial<List> = {}): List => ({
    items,
    loaded: true,
    statusKnown: true,
    seeding: false,
    heldNothing: false,
    ...over,
  });
  /** The ids a starting point holds, or null while there is none. */
  const held = (a: Announced) => (a && 'ids' in a ? a.ids.size : null);

  it('takes nothing from a list that has not loaded yet', () => {
    // The first render: no rows yet, and no status. Seeding from this was
    // what made every launch announce the inbox's unread mail as new.
    const step = announceStep(null, { ...loaded([]), loaded: false, statusKnown: false });
    expect(step.next).toBeNull();
    expect(step.fresh).toEqual([]);
  });

  it('takes the first loaded list silently, however much of it is unread', () => {
    const step = announceStep(null, loaded(unread13));
    expect(step.fresh).toEqual([]);
    expect(held(step.next)).toBe(13);
  });

  it('then announces what arrives after it', () => {
    const first = announceStep(null, loaded(unread13));
    const arrived = thread({ id: 900, date_ms: 20_000 });
    const step = announceStep(first.next, loaded([arrived, ...unread13]));
    expect(step.fresh.map((m) => m.id)).toEqual([900]);
  });

  it('does not announce an older page scrolled into the window', () => {
    const first = announceStep(null, loaded(unread13));
    const older = thread({ id: 901, date_ms: 1 });
    expect(announceStep(first.next, loaded([...unread13, older])).fresh).toEqual([]);
  });

  it("reads nothing from a window that is not the inbox's yet", () => {
    // Back from Receipts, from a search or from another account, the first
    // render still holds the old window's rows. Read as the inbox, the mail a
    // rule had filed in Receipts was announced as new.
    const first = announceStep(null, loaded(unread13));
    const filed = thread({ id: 902, date_ms: 30_000 });
    const step = announceStep(first.next, { ...loaded([filed, ...unread13]), loaded: false });
    expect(step.fresh).toEqual([]);
    expect(step.next).toBe(first.next);
  });

  it('waits for the status before deciding', () => {
    // Loaded before the status poll answered: whether the account holds any
    // mail, and whether its first pass is running, cannot be told yet.
    expect(announceStep(null, loaded([], { statusKnown: false })).next).toBeNull();
  });

  it('takes an empty inbox as the starting point at a launch, though its first pass runs', () => {
    // Inbox zero is the ordinary state here, and every launch's first pass
    // counts as seeding. Waiting it out took the overnight mail it brought
    // as the mailbox as it was, and announced none of it.
    const first = announceStep(null, loaded([], { seeding: true }));
    expect(held(first.next)).toBe(0);
    const arrived = thread({ id: 903, date_ms: 40_000 });
    const step = announceStep(first.next, loaded([arrived], { seeding: true }));
    expect(step.fresh.map((m) => m.id)).toEqual([903]);
  });

  it("takes a brand-new account's first rows silently", () => {
    // Nothing held at all: the first pass is the mailbox arriving, not mail.
    const fresh = { heldNothing: true, seeding: true };
    const first = announceStep(null, loaded([], fresh));
    expect(held(first.next)).toBeNull();
    const step = announceStep(first.next, loaded(unread13, fresh));
    expect(step.fresh).toEqual([]);
    expect(held(step.next)).toBe(13);
    // The pass ends; from there, what arrives is news.
    const over = announceStep(step.next, loaded(unread13));
    const arrived = thread({ id: 904, date_ms: 50_000 });
    expect(announceStep(over.next, loaded([arrived, ...unread13])).fresh.map((m) => m.id)).toEqual([904]);
  });

  it("keeps waiting through a brand-new account's first pass once it has begun", () => {
    // The status catches up as the pass stores mail: a count above nothing
    // must not end the wait while the inbox is still empty.
    const first = announceStep(null, loaded([], { heldNothing: true, seeding: true }));
    const later = announceStep(first.next, loaded([], { heldNothing: false, seeding: true }));
    expect(held(later.next)).toBeNull();
    const rows = announceStep(later.next, loaded(unread13, { seeding: true }));
    expect(rows.fresh).toEqual([]);
    expect(held(rows.next)).toBe(13);
  });

  it("takes a brand-new account's empty inbox once its first pass is over", () => {
    const first = announceStep(null, loaded([], { heldNothing: true, seeding: true }));
    const over = announceStep(first.next, loaded([], { heldNothing: true, seeding: false }));
    expect(held(over.next)).toBe(0);
    const arrived = thread({ id: 905, date_ms: 60_000 });
    expect(announceStep(over.next, loaded([arrived])).fresh.map((m) => m.id)).toEqual([905]);
  });

  it("takes in a brand-new account's whole first sync silently, to the end of its pass", () => {
    // Its pass files more of what it already held into the inbox, newer than
    // the first rows taken, and ends the same way: none of it is news.
    const fresh = { heldNothing: true, seeding: true };
    const first = announceStep(null, loaded([], fresh));
    const rows = announceStep(first.next, loaded(unread13, fresh));
    const more = thread({ id: 906, date_ms: 70_000 });
    const during = announceStep(rows.next, loaded([more, ...unread13], { seeding: true }));
    expect(during.fresh).toEqual([]);
    const last = thread({ id: 907, date_ms: 80_000 });
    const end = announceStep(during.next, loaded([last, more, ...unread13], { seeding: false }));
    expect(end.fresh).toEqual([]);
    // And from there, what arrives is news.
    const arrived = thread({ id: 908, date_ms: 90_000 });
    const after = announceStep(end.next, loaded([arrived, last, more, ...unread13]));
    expect(after.fresh.map((m) => m.id)).toEqual([908]);
  });

  it("announces what a later launch's first pass brings, to its end", () => {
    // The pass that ends in the same poll as the mail it brought: the end of
    // a pass that is not a first sync takes nothing in silently.
    const first = announceStep(null, loaded([], { seeding: true }));
    const arrived = thread({ id: 909, date_ms: 95_000 });
    const end = announceStep(first.next, loaded([arrived], { seeding: false }));
    expect(end.fresh.map((m) => m.id)).toEqual([909]);
  });

  it("waits for rows when a brand-new account's first pass was never seen running", () => {
    // A status left over from before the account was added says nothing is
    // seeding. Taking the empty inbox then announced the whole first sync.
    const first = announceStep(null, loaded([], { heldNothing: true, seeding: false }));
    const again = announceStep(first.next, loaded([], { heldNothing: true, seeding: false }));
    expect(held(again.next)).toBeNull();
    const rows = announceStep(again.next, loaded(unread13));
    expect(rows.fresh).toEqual([]);
    expect(held(rows.next)).toBe(13);
  });
});

describe('desktopNotice', () => {
  const words = {
    noSubject: '(no subject)',
    many: '13 new messages',
    fromPeople: (n: number) => `From ${n} people`,
    list: (names: string[]) => names.join(' + '),
  };

  it('names the sender and the subject of one message', () => {
    expect(desktopNotice([thread({ from_display: 'Sam', subject: 'Lunch' })], words)).toEqual({
      title: 'Sam',
      body: 'Lunch',
    });
  });

  it('names the sender of several from one person', () => {
    const batch = [thread({ from_display: 'Sam' }), thread({ from_display: 'Sam' })];
    expect(desktopNotice(batch, words)).toEqual({ title: 'Sam', body: '13 new messages' });
  });

  it('does not put one sender at the top of mail from several', () => {
    // The title was the first sender's name: "Sam Ortiz — 13 new messages",
    // over mail from four people.
    const batch = [
      thread({ from_display: 'Sam' }),
      thread({ from_display: 'Dana' }),
      thread({ from_display: 'Sam' }),
      thread({ from_display: '', from_addr: 'alex@example.com' }),
    ];
    expect(desktopNotice(batch, words)).toEqual({
      title: '13 new messages',
      body: 'Sam + Dana + alex@example.com',
    });
  });

  it('counts the people when there are too many to name', () => {
    const batch = ['A', 'B', 'C', 'D', 'E'].map((who) => thread({ from_display: who }));
    expect(desktopNotice(batch, words)).toEqual({ title: '13 new messages', body: 'From 5 people' });
  });
});

/**
 * Mail in an account that is not on screen. The window watched only the
 * inbox it shows; other clients announce every account's new mail. The shell
 * hands these over already unread and still in that inbox.
 */
describe('elsewhereNotices', () => {
  const arrived = [
    { account: 2, who: 'Riley Chen', subject: 'Lunch on Friday' },
    { account: 3, who: 'Dana Wu', subject: 'Board pack' },
    { account: 2, who: 'Sam Ortiz', subject: 'Re: Lunch on Friday' },
  ];

  it('says them account by account, in the order they came', () => {
    expect(elsewhereNotices(settings(), arrived, NOW)).toEqual([
      { account: 2, arrivals: [arrived[0], arrived[2]] },
      { account: 3, arrivals: [arrived[1]] },
    ]);
  });

  it('keeps quiet while notifications are paused or off', () => {
    expect(elsewhereNotices(settings({ notifyPausedUntil: String(NOW + 60_000) }), arrived, NOW)).toEqual([]);
    expect(elsewhereNotices(settings({ notifyLevel: 'none' }), arrived, NOW)).toEqual([]);
  });

  it('leaves them to the rules at the priority level', () => {
    // Priority means starred conversations, and these arrive with no star to
    // judge by: a rule that asks to be told is how that level hears them.
    expect(elsewhereNotices(settings({ notifyLevel: 'priority' }), arrived, NOW)).toEqual([]);
  });

  it('says nothing about nothing', () => {
    expect(elsewhereNotices(settings(), [], NOW)).toEqual([]);
  });
});
