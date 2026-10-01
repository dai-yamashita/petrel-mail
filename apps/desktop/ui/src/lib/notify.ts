import type { Settings } from './settings';
import type { Thread } from './api';

/**
 * Whether an interruption is warranted, and what to say.
 *
 * Kept out of the component because it is a rules question, not a rendering
 * one: pause, level and what counts as priority all have to agree, and three
 * components each deciding separately is how a paused app still buzzes.
 */

/** Priority mail, for the "priority only" level.
 *
 *  Deliberately conservative for now — starred, or addressed to you rather
 *  than a list. A wrong "priority" that stays silent costs you a message; a
 *  wrong one that fires costs the setting its credibility, and people switch
 *  notifications off entirely rather than tune them. */
export function isPriority(t: Thread): boolean {
  return t.starred;
}

export function shouldNotify(settings: Settings, now: number): boolean {
  if (settings.notifyLevel === 'none') return false;
  const until = Number(settings.notifyPausedUntil) || 0;
  return now >= until;
}

/** The rows that are new since the last look: not announced before, and no
 *  older than the newest thing announced so far. A page of older
 *  conversations scrolled into the window is the past, and without the
 *  second test it was announced as new mail. */
export function arrivalsSince(items: Thread[], announced: ReadonlySet<number>, floor: number): Thread[] {
  return items.filter((m) => !announced.has(m.id) && m.date_ms >= floor);
}

/** What the new-mail announcer has taken in. Null until it has decided
 *  anything for the account on screen; `waiting` through a brand-new
 *  account's first sync until its first rows come; then every row it has
 *  seen in the inbox, and the newest date among them. `firstSync` while the
 *  rows are still a brand-new account's first sync arriving. */
export type Announced =
  | null
  | { waiting: true; sawSeeding: boolean }
  | { ids: ReadonlySet<number>; newest: number; firstSync: boolean };

/**
 * One step of the new-mail announcer: the inbox as it now stands, and what in
 * it is new.
 *
 * The starting point is the inbox as first loaded, taken silently — the
 * mailbox as it already was. It used to be taken on the first render, before
 * any list had loaded, so it was an empty set and every launch announced the
 * inbox's unread mail as "13 new messages", desktop notification and all.
 * A list that is not the inbox's yet — the render that leaves a folder, a
 * search or another account still holds that window's rows — is no list at
 * all here; nor is anything decided before the status is known.
 *
 * Only an account that held nothing at all waits: its first sync is the
 * mailbox arriving, not mail, and its first rows are the starting point. It
 * waits until rows come, or until a first pass seen running is over with the
 * inbox still empty, and takes in the rest of that pass silently too, to its
 * end: a first sync files more of what it held into the inbox as it finishes.
 * Every other account starts from its inbox as loaded,
 * empty or not, though its first pass of the launch is running — inbox zero
 * is the ordinary state, and waiting that pass out took the overnight mail it
 * brought as the mailbox as it was, and announced none of it.
 */
export function announceStep(
  prior: Announced,
  list: {
    items: Thread[];
    /** The rows were loaded for the inbox as it is asked for now. */
    loaded: boolean;
    statusKnown: boolean;
    /** The account's first pass of this launch is running. */
    seeding: boolean;
    /** The account held no mail at all: added just now, or never synced. */
    heldNothing: boolean;
  },
): { next: Announced; fresh: Thread[] } {
  if (!list.loaded) return { next: prior, fresh: [] };
  const newestOf = (floor: number) => list.items.reduce((n, m) => Math.max(n, m.date_ms), floor);
  const start = (firstSync: boolean): { next: Announced; fresh: Thread[] } => ({
    next: { ids: new Set(list.items.map((m) => m.id)), newest: newestOf(0), firstSync },
    fresh: [],
  });
  if (prior === null) {
    if (!list.statusKnown) return { next: null, fresh: [] };
    if (list.heldNothing && list.items.length === 0) {
      return { next: { waiting: true, sawSeeding: list.seeding }, fresh: [] };
    }
    return start(list.heldNothing);
  }
  if ('waiting' in prior) {
    if (list.items.length > 0 || (prior.sawSeeding && !list.seeding)) {
      return start(list.seeding);
    }
    return { next: { waiting: true, sawSeeding: prior.sawSeeding || list.seeding }, fresh: [] };
  }
  const ids = new Set(prior.ids);
  for (const m of list.items) ids.add(m.id);
  const newest = newestOf(prior.newest);
  if (prior.firstSync) {
    // Still the first sync: taken in without a word, to the end of its pass.
    return { next: { ids, newest, firstSync: list.seeding }, fresh: [] };
  }
  const fresh = arrivalsSince(list.items, prior.ids, prior.newest);
  return { next: { ids, newest, firstSync: false }, fresh };
}

/** One message new to the inbox of an account that is not on screen, as the
 *  shell hands it over: already unread, and still in that inbox. */
export type Elsewhere = { account: number; who: string; subject: string };

/**
 * What to say about mail in accounts that are not on screen: the arrivals
 * that earn an interruption, account by account, in the order they came.
 *
 * Under the same pause and the same "nothing" as mail in the account on
 * screen. The priority level hears none of these: it means starred
 * conversations, and these arrive with no star to judge by, so a rule that
 * asks to be told is how that level hears another account's mail.
 */
export function elsewhereNotices(
  settings: Settings,
  arrivals: Elsewhere[],
  now: number,
): { account: number; arrivals: Elsewhere[] }[] {
  if (!shouldNotify(settings, now) || settings.notifyLevel === 'priority') return [];
  const out: { account: number; arrivals: Elsewhere[] }[] = [];
  for (const a of arrivals) {
    const group = out.find((g) => g.account === a.account);
    if (group) group.arrivals.push(a);
    else out.push({ account: a.account, arrivals: [a] });
  }
  return out;
}

/** How many senders a batch's notification names before it counts them. */
const NAMED_SENDERS = 3;

/**
 * A desktop notification for a batch of new mail: its title and its body.
 *
 * One message is its sender over its subject, and several from one person are
 * that person over the count. Several from several people are the count over
 * who they are from: the title used to be the first sender's name whoever
 * else had written, which read as one person sending all thirteen.
 */
export function desktopNotice(
  worth: Pick<Thread, 'from_display' | 'from_addr' | 'subject'>[],
  words: {
    noSubject: string;
    /** "13 new messages", already counted. */
    many: string;
    fromPeople: (count: number) => string;
    /** The locale's way of listing names. */
    list: (names: string[]) => string;
  },
): { title: string; body: string } {
  const who = (m: Pick<Thread, 'from_display' | 'from_addr'>) => m.from_display || m.from_addr;
  const top = worth[0];
  if (worth.length === 1) return { title: who(top), body: top.subject || words.noSubject };
  const senders = [...new Set(worth.map(who))];
  if (senders.length === 1) return { title: senders[0], body: words.many };
  return {
    title: words.many,
    body:
      senders.length <= NAMED_SENDERS ? words.list(senders) : words.fromPeople(senders.length),
  };
}

/** The conversations from a batch that earn an interruption. */
export function notifiable(settings: Settings, arrivals: Thread[], now: number): Thread[] {
  if (!shouldNotify(settings, now)) return [];
  const unread = arrivals.filter((t) => t.unread);
  return settings.notifyLevel === 'priority' ? unread.filter(isPriority) : unread;
}

/** What became of a notification. `reason` is for a person to read. */
export type NotifyResult = { ok: boolean; reason?: string };

/**
 * Posts an OS notification, if the user allows it and the OS agrees.
 *
 * Every failure here is non-fatal by design: notification permission can be
 * refused, revoked, or unavailable entirely (an unbundled build is a real
 * case). The in-app toast has already been shown by the time this runs, so a
 * refusal costs the user nothing.
 *
 * macOS goes through our own command rather than the plugin. The plugin's
 * macOS path ends at NSUserNotification, which macOS 26 no longer delivers,
 * and which reports success regardless — so the plugin cannot tell us anything
 * true there. See `src-tauri/src/notify.rs`. Windows and Linux keep the
 * plugin, where the OS story is sound.
 *
 * The result is returned rather than swallowed so that a caller which needs to
 * tell the user something — the settings pane's test button — has something
 * true to say.
 */
export async function postDesktopNotification(
  title: string,
  body: string,
): Promise<NotifyResult> {
  const isMac =
    typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || '');
  if (isMac) {
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('post_notification', { title, body });
      return { ok: true };
    } catch (e) {
      return { ok: false, reason: String(e) };
    }
  }
  try {
    const mod = await import('@tauri-apps/plugin-notification');
    let granted = await mod.isPermissionGranted();
    if (!granted) {
      granted = (await mod.requestPermission()) === 'granted';
    }
    if (!granted) return { ok: false, reason: 'permission-denied' };
    mod.sendNotification({ title, body });
    return { ok: true };
  } catch (e) {
    // Not running under Tauri, or the plugin is unavailable. Silence is the
    // right outcome for an arrival; the toast already happened.
    return { ok: false, reason: String(e) };
  }
}
