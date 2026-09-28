import { describe, expect, it, vi } from 'vitest';
import { LIST_PAGE } from './list-page';
import { DEFAULT_SORT } from './sort';
import type { Thread } from './api';
import {
  appendPage,
  firstPageCall,
  loadMoreCall,
  mailboxMoved,
  mergeHead,
  pageMore,
  refreshHead,
  renumbered,
  replaceLoadHasMore,
  runReplaceLoad,
  stillWanted,
  type Asked,
  type ThreadFetchers,
  type WindowSink,
} from './useThreadWindow';

let nextId = 1;

function thread(over: Partial<Thread> & Pick<Thread, 'thread_id'>): Thread {
  const id = over.id ?? nextId++;
  const from_display = over.from_display ?? 'Sender';
  const from_addr = over.from_addr ?? 'sender@example.com';
  const snippet = over.snippet ?? 'Snippet';
  const date_ms = over.date_ms ?? 1_000_000 - over.thread_id;
  return {
    id,
    thread_id: over.thread_id,
    newest: over.newest ?? { id, from_display, from_addr, snippet, date_ms, unread: false },
    from_display,
    from_addr,
    subject: over.subject ?? 'Subject',
    snippet,
    date_ms,
    message_count: over.message_count ?? 1,
    participants: over.participants ?? '',
    unread: over.unread ?? false,
    starred: over.starred ?? false,
    has_attachments: over.has_attachments ?? false,
    tags: over.tags ?? [],
    attachment_name: over.attachment_name ?? null,
    match_snippet: over.match_snippet ?? null,
    sort_value: over.sort_value ?? null,
  };
}

function threadsFetcher(pages: Record<string, Thread[][]>): ThreadFetchers['threads'] {
  return (
    view,
    offset,
    limit,
    sort,
    ascending,
    beforeDateMs,
    beforeThreadId,
  ) => {
    void offset;
    void sort;
    void ascending;
    const key =
      beforeDateMs === undefined && beforeThreadId === undefined
        ? `${view}:head`
        : `${view}:${beforeDateMs}:${beforeThreadId}`;
    const stack = pages[key] ?? [];
    const page = stack.shift() ?? [];
    void limit;
    return Promise.resolve(page);
  };
}

describe('firstPageCall', () => {
  it('asks for offset zero, LIST_PAGE rows, and no cursor', () => {
    expect(firstPageCall('inbox', DEFAULT_SORT)).toEqual([
      'inbox',
      0,
      LIST_PAGE,
      'date',
      false,
    ]);
  });
});

describe('loadMoreCall', () => {
  it('passes the last row as the keyset cursor', () => {
    const last = thread({ thread_id: 9, date_ms: 42 });
    expect(loadMoreCall('inbox', DEFAULT_SORT, last)).toEqual([
      'inbox',
      0,
      LIST_PAGE,
      'date',
      false,
      42,
      9,
    ]);
  });

  it('by sender or subject, also says what the last row showed', () => {
    // The conversation may sort elsewhere by the time the page is asked for,
    // and the next page has to start from where it was listed.
    const named = thread({ thread_id: 9, date_ms: 42, from_display: 'Sam Lee', from_addr: 'sam@example.com', subject: 'Plans' });
    const bare = thread({ thread_id: 9, date_ms: 42, from_display: '', from_addr: 'sam@example.com' });
    const at = ['inbox', 0, LIST_PAGE];
    expect(loadMoreCall('inbox', { key: 'sender', ascending: true }, named)).toEqual([...at, 'sender', true, 42, 9, 'Sam Lee']);
    expect(loadMoreCall('inbox', { key: 'sender', ascending: false }, bare)).toEqual([...at, 'sender', false, 42, 9, 'sam@example.com']);
    expect(loadMoreCall('inbox', { key: 'subject', ascending: true }, named)).toEqual([...at, 'subject', true, 42, 9, 'Plans']);
    // The engine's own value, when the row carries it, is what goes.
    const valued = thread({ thread_id: 9, date_ms: 42, from_display: 'Sam Lee', sort_value: 'bob' });
    expect(loadMoreCall('inbox', { key: 'sender', ascending: true }, valued)).toEqual([...at, 'sender', true, 42, 9, 'bob']);
  });
});

describe('appendPage', () => {
  it('appends without duplicate thread_ids', () => {
    const prev = [thread({ thread_id: 1 }), thread({ thread_id: 2 })];
    const incoming = [
      thread({ thread_id: 2, subject: 'dup' }),
      thread({ thread_id: 3 }),
    ];
    const { items, reachedEnd } = appendPage(prev, incoming);
    expect(items.map((t) => t.thread_id)).toEqual([1, 2, 3]);
    expect(items[1].subject).toBe(prev[1].subject);
    expect(reachedEnd).toBe(true);
  });

  it('reports the end when the page is short', () => {
    const prev = [thread({ thread_id: 1 })];
    const incoming = Array.from({ length: LIST_PAGE - 1 }, (_, i) =>
      thread({ thread_id: i + 10 }),
    );
    expect(appendPage(prev, incoming).reachedEnd).toBe(true);
  });

  it('keeps hasMore when a full page arrives', () => {
    const prev = [thread({ thread_id: 1 })];
    const incoming = Array.from({ length: LIST_PAGE }, (_, i) =>
      thread({ thread_id: i + 10 }),
    );
    expect(appendPage(prev, incoming).reachedEnd).toBe(false);
  });
});

describe('mergeHead', () => {
  const page = (ids: number[], first = 10_000) =>
    ids.map((tid, i) => thread({ thread_id: tid, date_ms: first - i }));
  const byDate = { byDate: true, ascending: false };

  it('leads with the fresh page and keeps the tail it does not cover', () => {
    const tail = thread({ thread_id: 300, subject: 'old tail', date_ms: 1 });
    const prev = [thread({ thread_id: 2, subject: 'stale', unread: true, date_ms: 9_999 }), tail];
    const incoming = page([1, 2, ...Array.from({ length: LIST_PAGE - 2 }, (_, i) => i + 3)]);
    const merged = mergeHead(prev, incoming, byDate);
    expect(merged.length).toBe(LIST_PAGE + 1);
    expect(merged[0].thread_id).toBe(1);
    expect(merged[1].subject).toBe('Subject');
    expect(merged[1].unread).toBe(false);
    expect(merged[merged.length - 1]).toBe(tail);
  });

  it('moves a conversation with a new reply to where the fresh page puts it', () => {
    const prev = page([5, 6, 7]);
    const incoming = page([7, 5, 6, ...Array.from({ length: LIST_PAGE - 3 }, (_, i) => i + 8)]);
    const merged = mergeHead(prev, incoming, byDate);
    expect(merged.slice(0, 3).map((t) => t.thread_id)).toEqual([7, 5, 6]);
    expect(merged.filter((t) => t.thread_id === 7).length).toBe(1);
  });

  it('drops a tail row the fresh page should have covered, by date', () => {
    const incoming = page(Array.from({ length: LIST_PAGE }, (_, i) => i + 1));
    const edge = incoming[incoming.length - 1].date_ms;
    const gone = thread({ thread_id: 900, date_ms: edge + 5 });
    const kept = thread({ thread_id: 901, date_ms: edge - 5 });
    const merged = mergeHead([gone, kept], incoming, byDate);
    expect(merged.some((t) => t.thread_id === 900)).toBe(false);
    expect(merged[merged.length - 1]).toBe(kept);
    // Not by date: there is no range to judge by, so both stay.
    const bySender = mergeHead([gone, kept], incoming, { byDate: false, ascending: false });
    expect(bySender.length).toBe(LIST_PAGE + 2);
  });

  it('treats a short page as the whole view', () => {
    const prev = page([1, 2, 3]);
    const incoming = page([1, 3]);
    expect(mergeHead(prev, incoming, byDate).map((t) => t.thread_id)).toEqual([1, 3]);
  });

  it('keeps the open conversation after the row it followed, whatever it now sorts by', () => {
    // Oldest first: it sat 51st, between the rows dated 1098 and 1100.
    const incoming = Array.from({ length: LIST_PAGE }, (_, i) =>
      thread({ thread_id: i + 1, date_ms: 1000 + 2 * i }),
    );
    const asc = { byDate: true, ascending: true };
    const held = thread({ thread_id: 500, date_ms: 1099 });
    const prev = [...incoming.slice(0, 50), held, ...incoming.slice(50)];
    expect(mergeHead(prev, incoming, asc, 500).indexOf(held)).toBe(50);
    // A refresh later, with the reply's date written in: that date is past
    // the page, and the row still stays where it was, not after the page.
    const replied = { ...held, id: 5001, date_ms: 9_999 };
    const prevReplied = [...incoming.slice(0, 50), replied, ...incoming.slice(50)];
    const again = mergeHead(prevReplied, incoming, asc, 500);
    expect(again.indexOf(replied)).toBe(50);
    expect(again.length).toBe(LIST_PAGE + 1);
    // Newest first, the same, inside the page's range.
    const desc = [...incoming].reverse();
    const prevDesc = [...desc.slice(0, 50), held, ...desc.slice(50)];
    expect(mergeHead(prevDesc, desc, byDate, 500).indexOf(held)).toBe(50);
    // By sender there is no range; it still stays where it was.
    const bySender = { byDate: false, ascending: true };
    expect(mergeHead(prevReplied, incoming, bySender, 500).indexOf(replied)).toBe(50);
    // Told nothing, the range drops it, and a row past the page goes after it.
    expect(mergeHead(prev, incoming, asc).includes(held)).toBe(false);
    expect(mergeHead(prevReplied, incoming, asc).indexOf(replied)).toBe(LIST_PAGE);
  });

  it('follows no row a reply moved, up the page or past it', () => {
    // Newest first, the open conversation last on the page. A new
    // conversation pushes it off, and the row above it takes a reply and goes
    // to the top: it stays at the bottom, after the row above that one.
    const loaded = page(Array.from({ length: LIST_PAGE }, (_, i) => i + 1));
    const open = loaded[LIST_PAGE - 1];
    const above = loaded[LIST_PAGE - 2];
    const arrived = thread({ thread_id: 900, date_ms: 30_000 });
    const replied = { ...above, id: 9_999, date_ms: 20_000 };
    const incoming = [arrived, replied, ...loaded.slice(0, LIST_PAGE - 2)];
    const merged = mergeHead(loaded, incoming, byDate, open.thread_id);
    expect(merged.indexOf(open)).toBe(LIST_PAGE);
    expect(merged[LIST_PAGE - 1].thread_id).toBe(loaded[LIST_PAGE - 3].thread_id);
    // By sender, the row right above it took a reply that sorts past the
    // page. With no range, its old row stays in the tail under its old id;
    // the open one does not follow it there.
    const bySender = { byDate: false, ascending: true };
    const prev = [...loaded.slice(0, 50), thread({ thread_id: 500 }), ...loaded.slice(50)];
    const fresh = [...loaded.slice(0, 49), ...loaded.slice(50), thread({ thread_id: 777 })];
    const tail = mergeHead(prev, fresh, bySender, 500);
    expect(tail.findIndex((t) => t.thread_id === 500)).toBe(49);
    expect(tail[48].thread_id).toBe(loaded[48].thread_id);
    expect(tail[tail.length - 1]).toBe(loaded[49]);
  });

  it('keeps to the tail when the rows after it went past the page too', () => {
    // A short window, the open conversation 55th, and 50 new conversations at
    // once: it and its neighbours are past the page now, still in order, and
    // J from it reaches the row that was after it.
    const old = Array.from({ length: 60 }, (_, i) => thread({ thread_id: i + 1, date_ms: 5_000 - i }));
    const arrived = Array.from({ length: 50 }, (_, i) =>
      thread({ thread_id: 1_000 + i, date_ms: 90_000 - i }),
    );
    const merged = mergeHead(old, [...arrived, ...old.slice(0, 50)], byDate, old[54].thread_id);
    const at = merged.indexOf(old[54]);
    expect(merged[at - 1]).toBe(old[53]);
    expect(merged[at + 1]).toBe(old[55]);
  });

  it('takes its place from the tail when the page no longer lists anything above it', () => {
    // A short window, the open conversation eleventh, and more than a page of
    // new mail at once: every row it followed is past the page now.
    const old = Array.from({ length: 20 }, (_, i) => thread({ thread_id: i + 1, date_ms: 5_000 - i }));
    const incoming = Array.from({ length: LIST_PAGE }, (_, i) =>
      thread({ thread_id: 1_000 + i, date_ms: 90_000 - i }),
    );
    const merged = mergeHead(old, incoming, byDate, old[10].thread_id);
    const at = merged.indexOf(old[10]);
    expect(merged[at - 1]).toBe(old[9]);
    expect(merged[at + 1]).toBe(old[11]);
  });

  it('goes to the top when nothing it followed is still listed', () => {
    const incoming = page(Array.from({ length: LIST_PAGE }, (_, i) => i + 1));
    const held = thread({ thread_id: 500, date_ms: 20_000 });
    const gone = thread({ thread_id: 900, date_ms: 19_000 });
    expect(mergeHead([gone, held], incoming, byDate, 500)[0]).toBe(held);
  });

  it('leaves the open conversation in the tail, in order, further down than the page', () => {
    // Newest first, page 2: the row above it took a reply and went to the
    // top. It stays among the rows around it rather than following.
    const loaded = page(Array.from({ length: LIST_PAGE }, (_, i) => i + 1));
    const edge = loaded[loaded.length - 1].date_ms;
    const above = thread({ thread_id: 800, date_ms: edge - 1 });
    const open = thread({ thread_id: 801, date_ms: edge - 2 });
    const below = thread({ thread_id: 802, date_ms: edge - 3 });
    const incoming = [{ ...above, id: 8000, date_ms: 20_000 }, ...loaded.slice(0, LIST_PAGE - 1)];
    const merged = mergeHead([...loaded, above, open, below], incoming, byDate, 801);
    expect(merged.slice(LIST_PAGE).map((t) => t.thread_id)).toEqual([LIST_PAGE, 801, 802]);
    expect(merged[0].thread_id).toBe(800);
  });
});

describe('renumbered', () => {
  it('names the new row of a conversation a reply landed in', () => {
    const open = thread({ thread_id: 7, id: 70 });
    const other = thread({ thread_id: 8, id: 80 });
    const replied = thread({ thread_id: 7, id: 71 });
    expect([...renumbered([open, other], [replied, other])]).toEqual([[70, 71]]);
  });

  it('follows a reply the head merge brings in', () => {
    const byDate = { byDate: true, ascending: false };
    const prev = [thread({ thread_id: 5, id: 50 }), thread({ thread_id: 6, id: 60 })];
    const reply = thread({ thread_id: 6, id: 61, date_ms: 9_000_000 });
    const merged = mergeHead(prev, [reply, prev[0]], byDate);
    expect([...renumbered(prev, merged)]).toEqual([[60, 61]]);
  });

  it('leaves a row alone while its id is still listed', () => {
    const a = thread({ thread_id: 1, id: 10 });
    const b = thread({ thread_id: 2, id: 20 });
    expect(renumbered([a, b], [b, a]).size).toBe(0);
    expect(renumbered([a, b], [{ ...a, unread: true }, b]).size).toBe(0);
  });

  it('has nothing for a conversation that left the list, or one new to it', () => {
    const gone = thread({ thread_id: 1, id: 10 });
    const stays = thread({ thread_id: 2, id: 20 });
    const fresh = thread({ thread_id: 3, id: 30 });
    expect(renumbered([gone, stays], [stays, fresh]).size).toBe(0);
  });
});

describe('runReplaceLoad', () => {
  it('loads the first listing page for an empty query', async () => {
    const rows = Array.from({ length: LIST_PAGE }, (_, i) => thread({ thread_id: i + 1 }));
    const threads = vi.fn(async () => rows);
    const search = vi.fn(async () => []);
    const fetchers: ThreadFetchers = { threads, search };

    const result = await runReplaceLoad(fetchers, '', 'inbox', DEFAULT_SORT);

    expect(threads).toHaveBeenCalledWith(...firstPageCall('inbox', DEFAULT_SORT));
    expect(search).not.toHaveBeenCalled();
    expect(result.items).toEqual(rows);
    expect(result.hasMore).toBe(true);
  });

  it('searches instead of listing when there is a query', async () => {
    const rows = [thread({ thread_id: 1 })];
    const threads = vi.fn(async () => rows);
    const search = vi.fn(async () => rows);
    const fetchers: ThreadFetchers = { threads, search };

    const result = await runReplaceLoad(fetchers, 'invoice', 'inbox', DEFAULT_SORT);

    expect(search).toHaveBeenCalledWith('invoice', 'date', false);
    expect(threads).not.toHaveBeenCalled();
    expect(result.hasMore).toBe(false);
  });

  it('replaces the list on a view change rather than merging', async () => {
    const inbox = [thread({ thread_id: 1, subject: 'inbox' })];
    const archive = [thread({ thread_id: 9, subject: 'archive' })];
    const fetchers: ThreadFetchers = {
      threads: threadsFetcher({
        'inbox:head': [inbox],
        'archive:head': [archive],
      }),
      search: async () => [],
    };

    const first = await runReplaceLoad(fetchers, '', 'inbox', DEFAULT_SORT);
    const second = await runReplaceLoad(fetchers, '', 'archive', DEFAULT_SORT);

    expect(first.items[0].subject).toBe('inbox');
    expect(second.items[0].subject).toBe('archive');
    expect(second.items.some((t) => t.thread_id === 1)).toBe(false);
  });

  it('clears hasMore when the first page is short', async () => {
    const fetchers: ThreadFetchers = {
      threads: async () => [thread({ thread_id: 1 })],
      search: async () => [],
    };
    const result = await runReplaceLoad(fetchers, '', 'inbox', DEFAULT_SORT);
    expect(replaceLoadHasMore('', result.items.length)).toBe(false);
    expect(result.hasMore).toBe(false);
  });
});

describe('loadMore integration', () => {
  it('requests the cursor from the last row and appends new pages', async () => {
    const page1 = Array.from({ length: LIST_PAGE }, (_, i) =>
      thread({ thread_id: i + 1, date_ms: 10_000 - i }),
    );
    const page2 = [thread({ thread_id: LIST_PAGE + 1, date_ms: 0 })];
    const threads = threadsFetcher({
      'inbox:head': [page1],
      [`inbox:${page1[page1.length - 1].date_ms}:${page1[page1.length - 1].thread_id}`]: [page2],
    });
    const fetchers: ThreadFetchers = { threads, search: async () => [] };

    const first = await runReplaceLoad(fetchers, '', 'inbox', DEFAULT_SORT);
    const last = first.items[first.items.length - 1];
    const more = await fetchers.threads(...loadMoreCall('inbox', DEFAULT_SORT, last));
    const { items, reachedEnd } = appendPage(first.items, more);

    expect(loadMoreCall('inbox', DEFAULT_SORT, last)).toEqual([
      'inbox',
      0,
      LIST_PAGE,
      'date',
      false,
      last.date_ms,
      last.thread_id,
    ]);
    expect(items.length).toBe(LIST_PAGE + 1);
    expect(reachedEnd).toBe(true);
  });
});

/* A sink that records what a background load did to the window, so a load
   can be shown to have left the rows alone. */
function recordingSink(initial: Thread[]) {
  let items = initial;
  const failures: string[] = [];
  const sink: WindowSink = {
    items: () => items,
    setItems: (next) => {
      items = typeof next === 'function' ? next(items) : next;
    },
    setHasMore: vi.fn(),
    setPageEnd: vi.fn(),
    bumpReplace: vi.fn(),
    failed: (e) => failures.push(e),
  };
  return { sink, rows: () => items, failures };
}

describe('mailboxMoved', () => {
  it('looks again when mail was filed elsewhere, though the count held', () => {
    // Another client moved a conversation into the folder on screen: the
    // same mail, in a different place. The count cannot see that.
    expect(mailboxMoved({ count: 120, gen: 4 }, { count: 120, gen: 5 })).toBe(true);
  });
  it('looks again when the count moves', () => {
    expect(mailboxMoved({ count: 120, gen: 4 }, { count: 121, gen: 4 })).toBe(true);
    expect(mailboxMoved({ count: 120, gen: 4 }, { count: 119, gen: 5 })).toBe(true);
  });
  it('leaves the window alone when nothing moved', () => {
    expect(mailboxMoved({ count: 120, gen: 4 }, { count: 120, gen: 4 })).toBe(false);
  });
  it('takes the first look as a baseline', () => {
    expect(mailboxMoved({ count: undefined, gen: undefined }, { count: 120, gen: 4 })).toBe(false);
  });
});

describe('stillWanted', () => {
  const asked: Asked = { gen: 3, view: 'inbox', sort: DEFAULT_SORT };
  it('accepts an answer for the same window', () => {
    expect(stillWanted(asked, { ...asked })).toBe(true);
  });
  it('drops an answer once the window has been replaced', () => {
    // A new account or query bumps the generation even when the view and
    // sort read the same, which is exactly the case the view check missed.
    expect(stillWanted(asked, { ...asked, gen: 4 })).toBe(false);
    expect(stillWanted(asked, { ...asked, view: 'sent' })).toBe(false);
    expect(stillWanted(asked, { ...asked, sort: { key: 'sender', ascending: true } })).toBe(false);
  });
});

describe('refreshHead', () => {
  const loaded = [thread({ thread_id: 1 }), thread({ thread_id: 2 })];

  it('keeps the loaded rows and reports the failure when the page cannot be fetched', async () => {
    const { sink, rows, failures } = recordingSink(loaded);
    const fetchers: ThreadFetchers = {
      threads: async () => {
        throw new Error('database is locked');
      },
      search: async () => [],
    };
    await refreshHead(fetchers, 'inbox', DEFAULT_SORT, () => true, sink);
    expect(rows()).toBe(loaded);
    expect(failures).toEqual(['Error: database is locked']);
  });

  it('folds a fresh page into the head when it arrives', async () => {
    const { sink, rows, failures } = recordingSink(loaded);
    const only = thread({ thread_id: 3 });
    const fetchers: ThreadFetchers = {
      threads: async () => [only],
      search: async () => [],
    };
    await refreshHead(fetchers, 'inbox', DEFAULT_SORT, () => true, sink);
    expect(rows().map((r) => r.thread_id)).toEqual([3]);
    expect(failures).toEqual([]);
    // A short page is the whole view, so it is where the next page starts.
    expect(sink.setPageEnd).toHaveBeenCalledWith(only);
  });

  it('drops an answer, and a failure, for a window since replaced', async () => {
    const { sink, rows, failures } = recordingSink(loaded);
    const gone = () => false;
    await refreshHead(
      { threads: async () => [thread({ thread_id: 9 })], search: async () => [] },
      'inbox', DEFAULT_SORT, gone, sink,
    );
    await refreshHead(
      { threads: async () => { throw new Error('late'); }, search: async () => [] },
      'inbox', DEFAULT_SORT, gone, sink,
    );
    expect(rows()).toBe(loaded);
    expect(failures).toEqual([]);
    expect(sink.bumpReplace).not.toHaveBeenCalled();
  });
});

describe('refreshHead and the open conversation', () => {
  // A full page, oldest first: dates 1000, 1002 .. 1198.
  const page = Array.from({ length: LIST_PAGE }, (_, i) =>
    thread({ thread_id: i + 1, id: i + 1, date_ms: 1000 + 2 * i }),
  );
  const oldestFirst = { key: 'date' as const, ascending: true };
  const bySender = { key: 'sender' as const, ascending: true };
  // Between the rows dated 1098 and 1100: the 51st row.
  const openRow = thread({ thread_id: 500, id: 5000, date_ms: 1099 });

  it('asks after it when a reply carries it past the page, and keeps its place', async () => {
    const { sink, rows } = recordingSink([...page.slice(0, 50), openRow, ...page.slice(50)]);
    const reply = thread({ thread_id: 500, id: 5001, date_ms: 9_999, message_count: 2 });
    const threadInView = vi.fn(async () => reply);
    await refreshHead(
      { threads: async () => page, search: async () => [], threadInView },
      'inbox', oldestFirst, () => true, sink, () => 500,
    );
    expect(threadInView).toHaveBeenCalledWith('inbox', 500);
    expect(rows().findIndex((r) => r.thread_id === 500)).toBe(50);
    expect(rows()[50].id).toBe(5001);
    expect(rows().filter((r) => r.thread_id === 500).length).toBe(1);
    // The next page is still asked for after the page's own last row, not
    // after the reply's date, which is the newest in the view.
    expect(sink.setPageEnd).not.toHaveBeenCalled();
  });

  it('keeps its place on the next refresh too, while it is still open', async () => {
    const { sink, rows } = recordingSink([...page.slice(0, 50), openRow, ...page.slice(50)]);
    const reply = thread({ thread_id: 500, id: 5001, date_ms: 9_999, message_count: 2 });
    const fetchers = { threads: async () => page, search: async () => [], threadInView: async () => reply };
    await refreshHead(fetchers, 'inbox', oldestFirst, () => true, sink, () => 500);
    await refreshHead(fetchers, 'inbox', oldestFirst, () => true, sink, () => 500);
    expect(rows().findIndex((r) => r.thread_id === 500)).toBe(50);
    // Once another conversation is open, it goes where its date puts it:
    // past this page, after it.
    await refreshHead(fetchers, 'inbox', oldestFirst, () => true, sink, () => 1);
    expect(rows().findIndex((r) => r.thread_id === 500)).toBe(LIST_PAGE);
  });

  it('never takes the next page from the row it keeps, even when that row is last', async () => {
    const last = thread({ thread_id: 500, id: 5000, date_ms: 1197 });
    const { sink, rows } = recordingSink([...page.slice(0, 99), last]);
    const reply = thread({ thread_id: 500, id: 5001, date_ms: 9_999, message_count: 2 });
    await refreshHead(
      { threads: async () => page, search: async () => [], threadInView: async () => reply },
      'inbox', oldestFirst, () => true, sink, () => 500,
    );
    expect(rows()[rows().length - 1].thread_id).toBe(100);
    expect(rows()[99].id).toBe(5001);
    expect(sink.setPageEnd).not.toHaveBeenCalled();
  });

  it('lets it go when the view no longer has it', async () => {
    const { sink, rows } = recordingSink([...page, openRow]);
    await refreshHead(
      { threads: async () => page, search: async () => [], threadInView: async () => null },
      'inbox', bySender, () => true, sink, () => 500,
    );
    expect(rows().some((r) => r.thread_id === 500)).toBe(false);
  });

  it('asks after it newest first too, where the page does not reach it', async () => {
    const newest = Array.from({ length: LIST_PAGE }, (_, i) =>
      thread({ thread_id: i + 1, id: i + 1, date_ms: 2000 - i }),
    );
    // Further down than the page: your own reply, filed in Sent, changed the
    // conversation's newest without moving its row.
    const below = thread({ thread_id: 500, id: 5000, date_ms: 1800 });
    const withReply = { ...below, message_count: 2, newest: { ...below.newest, id: 7000 } };
    const past = recordingSink([...newest, thread({ thread_id: 600, date_ms: 1850 }), below]);
    const asked = vi.fn(async () => withReply);
    await refreshHead(
      { threads: async () => newest, search: async () => [], threadInView: asked },
      'inbox', DEFAULT_SORT, () => true, past.sink, () => 500,
    );
    expect(asked).toHaveBeenCalledWith('inbox', 500);
    expect(past.rows().findIndex((r) => r.thread_id === 500)).toBe(LIST_PAGE + 1);
    expect(past.rows()[LIST_PAGE + 1].newest.id).toBe(7000);
    // Inside the page's range and not on it: gone from the view, and the
    // answer says so.
    const gone = thread({ thread_id: 500, id: 5000, date_ms: 1950 });
    const inside = recordingSink([...newest, gone]);
    await refreshHead(
      { threads: async () => newest, search: async () => [], threadInView: async () => null },
      'inbox', DEFAULT_SORT, () => true, inside.sink, () => 500,
    );
    expect(inside.rows().some((r) => r.thread_id === 500)).toBe(false);
  });

  it('does not ask when the page is the whole view, or already lists it', async () => {
    const threadInView = vi.fn(async () => null);
    const short = recordingSink([openRow]);
    await refreshHead(
      { threads: async () => [thread({ thread_id: 1 })], search: async () => [], threadInView },
      'inbox', oldestFirst, () => true, short.sink, () => 500,
    );
    const listed = recordingSink(page);
    await refreshHead(
      { threads: async () => page, search: async () => [], threadInView },
      'inbox', oldestFirst, () => true, listed.sink, () => 1,
    );
    expect(threadInView).not.toHaveBeenCalled();
  });

  it('drops the answer for a window since replaced', async () => {
    const before = [...page, openRow];
    const { sink, rows } = recordingSink(before);
    let live = true;
    await refreshHead(
      {
        threads: async () => page,
        search: async () => [],
        threadInView: async () => {
          live = false;
          return thread({ thread_id: 500, id: 5001, date_ms: 9_999 });
        },
      },
      'inbox', oldestFirst, () => live, sink, () => 500,
    );
    expect(rows().find((r) => r.thread_id === 500)?.id).toBe(5000);
  });
});

describe('pageMore', () => {
  const loaded = Array.from({ length: LIST_PAGE }, (_, i) => thread({ thread_id: i + 1 }));

  it('keeps the loaded rows and reports the failure when the next page cannot be fetched', async () => {
    const { sink, rows, failures } = recordingSink(loaded);
    await pageMore(
      { threads: async () => { throw new Error('connection reset'); }, search: async () => [] },
      'inbox', DEFAULT_SORT, loaded[loaded.length - 1], () => true, sink,
    );
    expect(rows()).toBe(loaded);
    expect(failures).toEqual(['Error: connection reset']);
    expect(sink.setHasMore).not.toHaveBeenCalled();
  });

  it('appends the page and notes the end of the list', async () => {
    const { sink, rows } = recordingSink(loaded);
    const next = thread({ thread_id: LIST_PAGE + 1 });
    await pageMore(
      { threads: async () => [next], search: async () => [] },
      'inbox', DEFAULT_SORT, loaded[loaded.length - 1], () => true, sink,
    );
    expect(rows().length).toBe(LIST_PAGE + 1);
    expect(sink.setHasMore).toHaveBeenCalledWith(false);
    expect(sink.setPageEnd).toHaveBeenCalledWith(next);
  });

  it('takes the next page from the last row a page delivered, even one already listed', async () => {
    // The page's last row is a conversation the window already has, so it is
    // not appended; the page still ended there.
    const listed = loaded[3];
    const { sink, rows } = recordingSink(loaded);
    await pageMore(
      { threads: async () => [thread({ thread_id: LIST_PAGE + 1 }), listed], search: async () => [] },
      'inbox', DEFAULT_SORT, loaded[loaded.length - 1], () => true, sink,
    );
    expect(rows().length).toBe(LIST_PAGE + 1);
    expect(sink.setPageEnd).toHaveBeenCalledWith(listed);
    // An empty page moves nothing.
    const empty = recordingSink(loaded);
    await pageMore(
      { threads: async () => [], search: async () => [] },
      'inbox', DEFAULT_SORT, loaded[loaded.length - 1], () => true, empty.sink,
    );
    expect(empty.sink.setPageEnd).not.toHaveBeenCalled();
  });

  it('drops a page for a window since replaced', async () => {
    const { sink, rows, failures } = recordingSink(loaded);
    await pageMore(
      { threads: async () => [thread({ thread_id: LIST_PAGE + 1 })], search: async () => [] },
      'inbox', DEFAULT_SORT, loaded[loaded.length - 1], () => false, sink,
    );
    expect(rows()).toBe(loaded);
    expect(failures).toEqual([]);
  });
});
