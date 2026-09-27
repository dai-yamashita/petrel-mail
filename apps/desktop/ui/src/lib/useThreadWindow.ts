import { useCallback, useEffect, useRef, useState } from 'react';
import type { Thread } from './api';
import { LIST_PAGE } from './list-page';
import { wireSort, type Sort } from './sort';

export type ThreadFetchers = {
  threads: (
    view: string,
    offset: number,
    limit: number,
    sort?: string,
    ascending?: boolean,
    beforeDateMs?: number,
    beforeThreadId?: number,
  ) => Promise<Thread[]>;
  search: (query: string, sort?: string, ascending?: boolean) => Promise<Thread[]>;
  /** One conversation as the view lists it; see `refreshHead`. */
  threadInView?: (view: string, threadId: number) => Promise<Thread | null>;
};

/** First listing page — offset zero, no keyset cursor. */
export function firstPageCall(view: string, sort: Sort): Parameters<ThreadFetchers['threads']> {
  const wire = wireSort(sort);
  return [view, 0, LIST_PAGE, wire.key, wire.ascending];
}

/** Next listing page — cursor taken from `last`, the row the page before it
 *  ended on. */
export function loadMoreCall(view: string, sort: Sort, last: Thread): Parameters<ThreadFetchers['threads']> {
  const wire = wireSort(sort);
  return [view, 0, LIST_PAGE, wire.key, wire.ascending, last.date_ms, last.thread_id];
}

export function replaceLoadHasMore(query: string, rowCount: number): boolean {
  return !query.trim() && rowCount === LIST_PAGE;
}

/** The mailbox changed under a loaded window: fold in a fresh first page.
 *
 *  The fresh page is the truth for everything it covers, so it goes first in
 *  its own order — a conversation that just gained a reply moves to the top
 *  rather than sitting where it was with a new dot on it — and the rows it
 *  covers leave their old places in the tail. A short page is the whole view,
 *  so the tail goes with it. A full page, sorted by date, also says which
 *  tail rows have gone: anything newer than its last row that it does not
 *  list was deleted or filed elsewhere by another client. Other sorts have no
 *  such range, so their tails are kept as they were.
 *
 *  `keep` is the open conversation, which the page does not list and which
 *  will be asked after by itself (see `refreshHead`). Its row stays, whatever
 *  the range says, and stays where it was — after the row it followed —
 *  whatever it now sorts by, for as long as it is open. Put after the page,
 *  the row being read went off the screen, J and K went on from there, and
 *  once its new date was written in, the next page was asked for after it.
 *  Further down than the first page, it is in the tail already, in order. */
export function mergeHead(
  prev: Thread[],
  incoming: Thread[],
  order: { byDate: boolean; ascending: boolean } = { byDate: false, ascending: false },
  keep: number | null = null,
): Thread[] {
  if (incoming.length < LIST_PAGE) return incoming;
  const covered = new Set(incoming.map((t) => t.thread_id));
  const edge = incoming[incoming.length - 1].date_ms;
  const insidePage = (t: Thread) =>
    order.byDate && (order.ascending ? t.date_ms < edge : t.date_ms > edge);
  const at =
    keep == null || covered.has(keep) ? -1 : prev.findIndex((t) => t.thread_id === keep);
  const held = at >= 0 && (at < incoming.length || insidePage(prev[at])) ? prev[at] : undefined;
  const rest = prev.filter((t) => t !== held && !covered.has(t.thread_id) && !insidePage(t));
  const merged = [...incoming, ...rest];
  if (!held) return merged;
  // After the nearest row above it that this refresh left where it was.
  //
  // A row a reply renumbered has moved, up the page or past it, and the row
  // being read went with it: to the top, out of sight, with J going on from
  // there. So a row counts only under the id it had. And where the rows
  // after it are still on the page, only a row still on the page counts: one
  // the page no longer lists went past it, and is stale in the tail. Where
  // those rows went past the page too — a short window that more than a page
  // of mail arrived in at once — its neighbours are tail rows, and they do.
  const listed = new Map(merged.map((t, i) => [t.thread_id, i]));
  const unmoved = (t: Thread): number => {
    const j = listed.get(t.thread_id);
    return j !== undefined && merged[j].id === t.id ? j : -1;
  };
  const below = prev.slice(at + 1).find((t) => unmoved(t) >= 0);
  const onPage = below === undefined || covered.has(below.thread_id);
  let to = 0;
  for (let i = at - 1; i >= 0; i -= 1) {
    const j = unmoved(prev[i]);
    if (j >= 0 && (!onPage || covered.has(prev[i].thread_id))) {
      to = j + 1;
      break;
    }
  }
  return [...merged.slice(0, to), held, ...merged.slice(to)];
}

/** Which rows came back under a new id, old id to new.
 *
 *  A conversation is listed under its newest message in the view, so a reply
 *  landing in one gives its row a new id. The conversation is the same one,
 *  and whatever pointed at the old row — the one open, the selection — should
 *  point at the new. A row whose id is still listed has not moved, and a
 *  conversation that left the list has nothing to point at. Only for lists of
 *  conversations: in Drafts a row is a message, and two can share a thread. */
export function renumbered(prev: Thread[], next: Thread[]): Map<number, number> {
  const ids = new Set(next.map((t) => t.id));
  const byThread = new Map(next.map((t) => [t.thread_id, t.id]));
  const moved = new Map<number, number>();
  for (const t of prev) {
    if (ids.has(t.id)) continue;
    const now = byThread.get(t.thread_id);
    if (now != null) moved.set(t.id, now);
  }
  return moved;
}

export function appendPage(
  prev: Thread[],
  incoming: Thread[],
): { items: Thread[]; reachedEnd: boolean } {
  const seen = new Set(prev.map((t) => t.thread_id));
  const append = incoming.filter((t) => !seen.has(t.thread_id));
  return { items: [...prev, ...append], reachedEnd: incoming.length < LIST_PAGE };
}

export async function runReplaceLoad(
  fetchers: ThreadFetchers,
  query: string,
  view: string,
  sort: Sort,
): Promise<{ items: Thread[]; hasMore: boolean }> {
  const trimmed = query.trim();
  const wire = wireSort(sort);
  if (trimmed) {
    const items = await fetchers.search(trimmed, wire.key, wire.ascending);
    return { items, hasMore: false };
  }
  const items = await fetchers.threads(...firstPageCall(view, sort));
  return { items, hasMore: replaceLoadHasMore(query, items.length) };
}

/** Where the mailbox stood at a look: how much mail, and how many times a
 *  sync has moved any. */
export type MailboxMark = { count: number | undefined; gen: number | undefined };

/** Whether the mailbox changed under the window since the last look. The
 *  count moves when mail arrives or is deleted; the generation moves when
 *  mail is filed in or out elsewhere, which moves no count. The first look
 *  is a baseline, not a change. */
export function mailboxMoved(prev: MailboxMark, next: MailboxMark): boolean {
  if (prev.count === undefined || next.count === undefined) return false;
  return next.count !== prev.count || next.gen !== prev.gen;
}

/** What a load was asked for, so its answer can be checked against what the
 *  window wants by the time it lands. The generation counts every replaced
 *  window — account, query, view or sort — so a page for a list since left
 *  is never folded into the one showing now. The view and sort are checked
 *  as well as the generation because the merge and the page are not started
 *  by the replace effect, and only the generation ties them to it. */
export type Asked = { gen: number; view: string; sort: Sort };

export function stillWanted(asked: Asked, now: Asked): boolean {
  return asked.gen === now.gen && asked.view === now.view && asked.sort === now.sort;
}

/** Where a background load puts its answer. The rows it reads and writes
 *  are the window's; a failure is reported and changes nothing else. */
export type WindowSink = {
  items: () => Thread[];
  setItems: (next: Thread[] | ((prev: Thread[]) => Thread[])) => void;
  setHasMore: (more: boolean) => void;
  /** Where the next page starts: the last row a page delivered, as it was
   *  delivered. Not the window's last row, which can be the open
   *  conversation kept in place with a date from somewhere else in the view:
   *  asking for the page after that one, the newest, got nothing and ended
   *  the list. */
  setPageEnd: (row: Thread | null) => void;
  bumpReplace: () => void;
  /** A background load that could not be made. The rows on screen are still
   *  the rows; the notice says the newest may be missing. */
  failed: (error: string) => void;
};

/** Folds a fresh first page into the loaded window.
 *
 *  Its failure keeps the rows. A refresh that fails used to put the whole
 *  list behind an error notice — forty conversations gone because one poll
 *  could not get a page — when everything on screen was still true. */
export async function refreshHead(
  fetchers: ThreadFetchers,
  view: string,
  sort: Sort,
  wanted: () => boolean,
  sink: WindowSink,
  openThread: () => number | null = () => null,
): Promise<void> {
  try {
    const rows = await fetchers.threads(...firstPageCall(view, sort));
    if (!wanted()) return;
    const wasEmpty = sink.items().length === 0;
    // The open conversation, where a full first page does not reach it. A
    // page that misses it says nothing true about it. Sorted by anything but
    // newest first, a reply can carry it past the page, where its row was
    // taken for gone, closing it on the person reading it, or kept stale, so
    // the reply never showed. Newest first, a message joining it outside the
    // view — your own reply, filed in Sent — changes the row without moving
    // it, and further down than the page, that never showed either. So its
    // row stays, and is asked after by itself.
    const open = openThread();
    const askAfter =
      open != null &&
      fetchers.threadInView != null &&
      rows.length === LIST_PAGE &&
      !rows.some((t) => t.thread_id === open);
    sink.setItems((cur) =>
      mergeHead(
        cur,
        rows,
        { byDate: sort.key === 'date', ascending: sort.ascending },
        askAfter ? open : null,
      ),
    );
    // A full page leaves the next one where the last page left it; the rows
    // past this one are still those.
    if (wasEmpty || rows.length < LIST_PAGE) sink.setPageEnd(rows[rows.length - 1] ?? null);
    if (wasEmpty) {
      sink.setHasMore(rows.length === LIST_PAGE);
      sink.bumpReplace();
    }
    if (!askAfter || open == null || !fetchers.threadInView) return;
    const fresh = await fetchers.threadInView(view, open);
    if (!wanted()) return;
    sink.setItems((cur) => {
      const at = cur.findIndex((t) => t.thread_id === open);
      if (at < 0) return cur;
      if (fresh == null) return [...cur.slice(0, at), ...cur.slice(at + 1)];
      if (JSON.stringify(fresh) === JSON.stringify(cur[at])) return cur;
      const next = cur.slice();
      next[at] = fresh;
      return next;
    });
  } catch (err: unknown) {
    if (wanted()) sink.failed(String(err));
  }
}

/** Appends the page after `last`. Same rule as the head merge: a page for a
 *  window since left is dropped, and a failure keeps what is loaded. */
export async function pageMore(
  fetchers: ThreadFetchers,
  view: string,
  sort: Sort,
  last: Thread,
  wanted: () => boolean,
  sink: WindowSink,
): Promise<void> {
  try {
    const rows = await fetchers.threads(...loadMoreCall(view, sort, last));
    if (!wanted()) return;
    const { items: next, reachedEnd } = appendPage(sink.items(), rows);
    sink.setItems(next);
    if (rows.length > 0) sink.setPageEnd(rows[rows.length - 1]);
    if (reachedEnd) sink.setHasMore(false);
  } catch (err: unknown) {
    if (wanted()) sink.failed(String(err));
  }
}

export function useThreadWindow(args: {
  query: string;
  view: string;
  sort: Sort;
  accountEpoch: number;
  /** Live message count. Increases mean new mail — merge into the head, never replace the loaded window. */
  messageCount: number | undefined;
  /** Moves when a sync moved, removed or reflagged mail. A move made in
   *  another client changes no count — the same mail, somewhere else — so
   *  this is what tells the folder on screen that mail arrived or left. */
  mailGen?: number;
  fetchers: ThreadFetchers;
  /** A background page or refresh that failed. The rows stay; this is where
   *  the failure is said. */
  onRefreshFailed?: (error: string) => void;
  /** The conversation open in the reader, read when a refresh lands; see
   *  `refreshHead`. */
  openThread?: () => number | null;
  /** False until the saved settings are in. The first window waits for them:
   *  loaded in the default order, its first row, the newest conversation,
   *  became the open one, and was kept when the saved order replaced the
   *  window a moment later, scrolled to wherever that order put it. */
  ready?: boolean;
}): {
  items: Thread[];
  setItems: React.Dispatch<React.SetStateAction<Thread[]>>;
  loading: boolean;
  /** Why the window could not be loaded at all. Only a replace load sets
   *  it — a window that never arrived — and the next replace clears it. */
  error: string | null;
  hasMore: boolean;
  loadMore: () => void;
  /** Bumps when the loaded window is replaced (view, query, sort, account)
   *  or when the first mail lands in an empty list. Paging and new mail at
   *  the head do not bump it — the highlight must not jump just because
   *  the array is new. */
  replaceEpoch: number;
} {
  const {
    query,
    view,
    sort,
    accountEpoch,
    messageCount,
    mailGen,
    fetchers,
    onRefreshFailed,
    openThread,
    ready = true,
  } = args;

  const [items, setItems] = useState<Thread[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);

  const itemsRef = useRef(items);
  itemsRef.current = items;

  const queryRef = useRef(query);
  queryRef.current = query;

  const hasMoreRef = useRef(hasMore);
  hasMoreRef.current = hasMore;

  const fetchersRef = useRef(fetchers);
  fetchersRef.current = fetchers;

  const viewRef = useRef(view);
  viewRef.current = view;

  const sortRef = useRef(sort);
  sortRef.current = sort;

  const failedRef = useRef(onRefreshFailed);
  failedRef.current = onRefreshFailed;

  const openThreadRef = useRef(openThread);
  openThreadRef.current = openThread;

  const readyRef = useRef(ready);
  readyRef.current = ready;

  // The row the last page ended on; see `WindowSink.setPageEnd`.
  const pageEnd = useRef<Thread | null>(null);

  const loadMoreInFlight = useRef(false);
  const messageCountRef = useRef(messageCount);
  const mailGenRef = useRef(mailGen);
  const [replaceEpoch, setReplaceEpoch] = useState(0);
  // Counts replaced windows. Every load remembers the generation it was
  // started under and is dropped if the window has been replaced since.
  const gen = useRef(0);

  const asked = useCallback(
    (): Asked => ({ gen: gen.current, view: viewRef.current, sort: sortRef.current }),
    [],
  );

  const sink = useRef<WindowSink>({
    items: () => itemsRef.current,
    setItems: (next) => setItems(next),
    setHasMore: (more) => setHasMore(more),
    setPageEnd: (row) => {
      pageEnd.current = row;
    },
    bumpReplace: () => setReplaceEpoch((n) => n + 1),
    failed: (e) => failedRef.current?.(e),
  });

  // Replace the window when the mailbox, query, sort, or account changes.
  useEffect(() => {
    if (!ready) return;
    let live = true;
    gen.current += 1;
    const myGen = gen.current;
    setLoading(true);
    // A failure belongs to the window that failed. Left standing, it hid
    // the next window too, even when that one loaded.
    setError(null);

    // 150ms while a query is present, 0 for a mailbox switch — that one is a
    // click and has nothing to wait for.
    //
    // A debounce only helps when it is longer than the gaps between keystrokes:
    // at a steady cadence, a gap wider than the delay fires a search on every
    // key, and a gap narrower than it fires one at the end. At 100ms only a fast
    // typist got that; ordinary typing sits at 150–200ms between keys, and a
    // query carrying punctuation — `from:sam` — is slower still, so most queries
    // were searched once per character. The 50ms this adds to settling is paid
    // once; at a hundred thousand messages a bracketed boolean query costs 93ms
    // each time it runs, which is what was being spent per keystroke and thrown
    // away — the fetch is generation-guarded, so those results were discarded,
    // not shown. Past about 200ms the field stops feeling live, which is the
    // other wall.
    const debounceMs = query.trim() ? 150 : 0;
    const handle = window.setTimeout(() => {
      runReplaceLoad(fetchersRef.current, query, view, sort)
        .then(({ items: rows, hasMore: more }) => {
          if (!live || gen.current !== myGen) return;
          setItems(rows);
          pageEnd.current = rows[rows.length - 1] ?? null;
          setHasMore(more);
          setReplaceEpoch((n) => n + 1);
          setLoading(false);
        })
        .catch((err: unknown) => {
          if (!live || gen.current !== myGen) return;
          setError(String(err));
          setLoading(false);
        });
    }, debounceMs);

    return () => {
      live = false;
      window.clearTimeout(handle);
    };
  }, [query, view, sort, accountEpoch, ready]);

  // On a reset, remember the count without treating it as new mail.
  useEffect(() => {
    messageCountRef.current = messageCount;
    mailGenRef.current = mailGen;
    // `messageCount` is the snapshot we store, not a trigger. Including it
    // would collapse "new mail" into "the mailbox changed" and skip the merge.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, view, sort, accountEpoch]);

  // A changed count or generation with no search running: fold a fresh first
  // page into the window. Up is new mail; down is something deleted
  // elsewhere, and the row it left behind should go the same way; the
  // generation alone is mail filed in or out by another client.
  useEffect(() => {
    if (messageCount === undefined || query.trim()) return;

    const prev = { count: messageCountRef.current, gen: mailGenRef.current };
    messageCountRef.current = messageCount;
    mailGenRef.current = mailGen;
    if (!mailboxMoved(prev, { count: messageCount, gen: mailGen })) return;
    // Before the first window: it will read the mailbox as it is now.
    if (!readyRef.current) return;

    let live = true;
    // The answer belongs to the window asked for. A page for the inbox that
    // arrived after a click on Sent used to be merged into Sent, and a short
    // one replaced it outright; one for the last account did the same.
    const was = asked();
    void refreshHead(
      fetchersRef.current,
      was.view,
      was.sort,
      () => live && stillWanted(was, asked()),
      sink.current,
      () => openThreadRef.current?.() ?? null,
    );

    return () => {
      live = false;
    };
  }, [messageCount, mailGen, query, asked]);

  const loadMore = useCallback(() => {
    if (queryRef.current.trim() || !hasMoreRef.current || loadMoreInFlight.current) return;

    const current = itemsRef.current;
    const last = pageEnd.current ?? current[current.length - 1];
    if (!last) return;

    loadMoreInFlight.current = true;
    const was = asked();
    void pageMore(
      fetchersRef.current,
      was.view,
      was.sort,
      last,
      () => stillWanted(was, asked()),
      sink.current,
    ).finally(() => {
      loadMoreInFlight.current = false;
    });
  }, [asked]);

  return { items, setItems, loading, error, hasMore, loadMore, replaceEpoch };
}
