import { useEffect, useRef } from 'react';

export type KeyActions = {
  /** `rowPressed`: the Enter was on a row of the list, which Ariakit has
   *  already pressed as a click — selecting it, and in Drafts resuming it. */
  openConversation: (rowPressed: boolean) => void;
  backToList: () => void;
  cyclePanes: (backwards: boolean) => void;
  goTo: (view: string) => void;
  switchAccount: (index: number) => void;
  openPalette: () => void;
  openHelp: () => void;
  openSettings: () => void;
  focusSearch: () => void;
  triage: (kind: import('./api').ActionKind) => void;
  compose: () => void;
  reply: (all: boolean) => void;
  forward: () => void;
  snooze: () => void;
  select: () => void;
  extendSelection: (down: boolean) => void;
  clearSelection: () => void;
  openMove: () => void;
  openTag: () => void;
  toggleStar: () => void;
  moveToInbox: () => void;
  popOut: () => void;
  toggleReaderFull: () => void;
  findInMessage: () => void;
  /** Saves the search in the field, when there is one to save. */
  saveSearch: () => void;
  undo: () => void;
};

/** True while a modal or a menu is up. Single-key commands must not reach the
 *  list behind it: archiving a conversation you cannot see, with the toast
 *  hidden under the dialog, is the worst possible version of a shortcut
 *  firing. A menu counts for the same reason — a right-click menu offering
 *  Archive was open over the row while E archived it underneath, and the menu
 *  then acted on a row that had already gone.
 *
 *  The `:not([hidden])` is load-bearing. Ariakit keeps every dialog and menu
 *  mounted and marks the closed ones `hidden`, so a bare `[role="dialog"]`
 *  matches even when nothing is open — a guard that would silently disable
 *  every shortcut in the app, permanently. */
function modalOpen(): boolean {
  return document.querySelector('[role="dialog"]:not([hidden]), [role="menu"]:not([hidden])') !== null;
}

/** A modal dialog is up — Settings, a picker, a confirmation: something that
 *  holds the pointer and the keys until it closes, so another dialog cannot
 *  usefully open over it. Ariakit marks its dialogs no differently from its
 *  popovers, but while a modal one is open it makes the rest of the page
 *  inert, which a menu or a popover never does, and nothing else here does.
 *  Two open at once each made the other inert: neither took a click, and
 *  Escape closed neither. */
export function dialogOpen(): boolean {
  return (
    document.querySelector('[role="dialog"]:not([hidden])') !== null &&
    document.querySelector('[inert]') !== null
  );
}

/** Where a keystroke means text, not a command. */
function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  return (
    el.tagName === 'INPUT' ||
    el.tagName === 'TEXTAREA' ||
    el.tagName === 'SELECT' ||
    el.isContentEditable
  );
}

/** Where a key was pressed, as far as single-key commands care: one of the
 *  panes they belong to, the composer, or somewhere else — usually the page
 *  itself, which is where focus falls when it leaves a field. */
export type KeyPlace = 'pane' | 'composer' | 'elsewhere';

/** Where a key was pressed. `active` is where focus is, for a key with no
 *  element of its own: a key pressed in a message is forwarded out of the
 *  sandboxed frame and replayed on the window, which sits in no pane, and was
 *  read as pressed nowhere — while a reply was open, every key pressed in the
 *  message it answered was refused. It was pressed where focus is, which is
 *  the message's frame, in the reader. */
export function placeOf(
  target: EventTarget | null,
  active: Element | null = document.activeElement,
): KeyPlace {
  const el =
    typeof (target as Element | null)?.closest === 'function' ? (target as Element) : active;
  if (el?.closest?.('.compose')) return 'composer';
  if (el?.closest?.('.rail, .list-pane, .reader')) return 'pane';
  return 'elsewhere';
}

/** The docked composer is open in this window. */
function composerOpen(): boolean {
  return document.querySelector('.compose') !== null;
}

/**
 * Whether a single-key mail command may run.
 *
 * Never from inside the composer, and while it is open, only from the panes
 * the commands belong to. Focus leaves a text field more ways than it looks:
 * Tab from Subject went to the formatting toolbar, Escape blurred to the page,
 * and in WebKit a click on Attach dropped it there. The next letters were
 * meant for the message, and ran as commands on the conversation behind it —
 * "Hi Dana" opened Reply all at its "a" and put the rest of the sentence in
 * the reply, and "see" starred and archived two conversations. Apple Mail,
 * Thunderbird and Gmail never let keys typed while writing reach the mailbox.
 *
 * F6 is the exception: it moves between panes, and is the way back to them.
 */
export function singleKeysAllowed(composer: boolean, place: KeyPlace, key: string): boolean {
  if (key === 'F6') return true;
  if (place === 'composer') return false;
  return !composer || place === 'pane';
}

/** Everything the single-key rule weighs, read from the page by
 *  `mailKeyAllowed`. */
export type KeyMoment = {
  /** The key was typed into a field. */
  typing: boolean;
  /** A dialog or a menu is open. */
  modal: boolean;
  /** The docked composer is open. */
  composer: boolean;
  place: KeyPlace;
};

/** The rule itself, as a function of what was read: see `mailKeyAllowed`. */
export function mailKeyDecision(m: KeyMoment, key: string): boolean {
  if (m.typing || m.modal) return false;
  return singleKeysAllowed(m.composer, m.place, key);
}

/**
 * Whether an unmodified key may act as a mail command now.
 *
 * The one decision for every listener that takes a single key — this hook's,
 * the list's J and K and the reader's [ and ]. Each used to keep its own
 * conditions, and the list's and the reader's never learnt the composer rule:
 * with focus on the composer's toolbar or its From line, "Thanks so much"
 * walked the list at its k, starred and popped out the conversation it
 * landed on, and replaced the message being written at its c.
 */
export function mailKeyAllowed(e: KeyboardEvent): boolean {
  return mailKeyDecision(
    {
      typing: isTyping(e.target),
      modal: modalOpen(),
      composer: composerOpen(),
      place: placeOf(e.target),
    },
    e.key,
  );
}

/** Whether Escape in a text field gives the field up. It does in the search
 *  field, where it is how you leave; in the composer it dropped focus to the
 *  page, which is where the next letters became commands. */
export function escapeLeavesField(place: KeyPlace): boolean {
  return place !== 'composer';
}

/** Whether ⌘1–⌘9 may switch accounts now.
 *
 *  Not under an open dialog, as ⌘K and ⌘, are not: a confirmation opened on
 *  one account outlived the switch and then acted on the other — Empty Trash
 *  expunged the other account's Trash, permanently. Not while a recipient is
 *  being typed in the composer either: switching saves and closes the draft,
 *  and the address still in the field would go with the window. */
export function accountSwitchAllowed(dialogUp: boolean, typingInComposer: boolean): boolean {
  return !dialogUp && !typingInComposer;
}

/** True when the focused thing already answers to Enter on its own.
 *
 *  Enter is how a button is pressed. This handler claimed it for "open the
 *  selected conversation" and cancelled the keydown, which cancelled the click
 *  the browser was about to synthesise — so every button in the window did
 *  nothing on Enter and worked only on Space, while Enter silently opened
 *  whatever the list had selected. `isTyping` guarded text fields and nothing
 *  guarded controls.
 *
 *  Menu items are included for completeness; Ariakit's own handler takes those
 *  first, because a menu makes `modalOpen()` true. Options too: the composer's
 *  font and size lists are listboxes, not menus, so Enter on one of their
 *  options came through as "open the conversation" — which in Drafts reloaded
 *  the draft under the person typing it. The message list's own rows are
 *  options as well, and `listRow` answers for them first. */
function activatable(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return (
    el?.closest?.(
      'button, a[href], summary, [role="button"], [role="link"], [role="menuitem"],' +
        ' [role="menuitemradio"], [role="menuitemcheckbox"], [role="checkbox"], [role="tab"],' +
        ' [role="option"]',
    ) != null
  );
}

/** True when the Enter is on a row of the message list itself.
 *
 *  A row is a button to the DOM, so `activatable` counted it as answering
 *  Enter on its own — and it does, but only as a click: Ariakit presses the
 *  active row, which selects it. Selecting is not opening. With the reading
 *  pane off, Enter is the only way to see a message at all, and it did
 *  nothing. The row itself only; a control inside a row keeps its own Enter. */
function listRow(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return el?.matches?.('.row[role="option"]') ?? false;
}

const GOTO: Record<string, string> = {
  i: 'inbox',
  s: 'starred',
  t: 'sent',
  d: 'drafts',
  // Gmail's own chord for All Mail.
  a: 'all-mail',
};

/**
 * One listener for every global shortcut, so bindings cannot drift apart across
 * components — and so the "single-key shortcuts pause while typing" rule is
 * enforced in one place rather than remembered in each handler.
 */
export function useKeyboard(actions: KeyActions) {
  const ref = useRef(actions);
  ref.current = actions;
  // A pending `g` waiting for its second key. Cleared on a timeout so a stray
  // press does not silently swallow the next keystroke minutes later.
  const chord = useRef<{ key: string; at: number } | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const a = ref.current;
      const typing = isTyping(e.target);
      const mod = e.metaKey || e.ctrlKey;

      // Modified shortcuts work everywhere, including in a text field: ⌘K is
      // how you get *out* of one.
      if (mod) {
        if (!e.altKey) {
          const k = e.key.toLowerCase();
          // Not over another dialog. The palette opened on top of Settings
          // there, where the dialog underneath held the pointer and Enter did
          // nothing; and with the palette itself already open, it is open.
          if (k === 'k') {
            e.preventDefault();
            if (!dialogOpen()) a.openPalette();
            return;
          }
          if (k === ',') {
            e.preventDefault();
            if (!dialogOpen()) a.openSettings();
            return;
          }
          if (/^[1-9]$/.test(e.key)) {
            e.preventDefault();
            const inComposer = isTyping(e.target) && placeOf(e.target) === 'composer';
            if (!accountSwitchAllowed(dialogOpen(), inComposer)) return;
            return a.switchAccount(Number(e.key));
          }
          // Free because modified keys stopped falling through to the
          // single-key commands; before that this forwarded the message.
          if (k === 'f') return e.preventDefault(), a.findInMessage();
          // ⌘⇧S saves the search in the field. Modified, because it has to work
          // *while typing in the field* — an unmodified key never can — and S
          // for save, shifted so it cannot be mistaken for the ⌘S that most
          // apps spend on a document.
          if (k === 's' && e.shiftKey) return e.preventDefault(), a.saveSearch();
        }
        // Anything else held with ⌘ or ctrl belongs to the system, and this
        // return is the whole of what makes that true.
        //
        // Without it every modified key fell through to the single-key commands
        // below: ⌘C opened the composer, ⌘A replied to all, ⌘V opened the move
        // picker, ⌘Z undid a triage action and ⌘F forwarded. Worse, they call
        // preventDefault, so the system action was not merely shadowed but
        // swallowed — you could not copy text out of a message at all.
        return;
      }

      if (typing) {
        if (e.key === 'Escape' && escapeLeavesField(placeOf(e.target))) {
          (e.target as HTMLElement).blur();
        }
        return;
      }

      // Under a dialog or a menu, and while a message is being written, the
      // rule every single key obeys: see `mailKeyAllowed`. Escape still
      // belongs to an open dialog, which handles it itself.
      if (!mailKeyAllowed(e)) return;

      // A pending chord takes the next key, if it arrives promptly.
      if (chord.current) {
        const pending = chord.current;
        chord.current = null;
        if (Date.now() - pending.at < 1500 && pending.key === 'g') {
          const view = GOTO[e.key.toLowerCase()];
          if (view) {
            e.preventDefault();
            a.goTo(view);
            return;
          }
        }
      }

      if ('eE#!sSzZIUvVlLcCrRaAfFbBxXJK'.includes(e.key)) {
        void import('./api').then(({ api }) =>
          api.log(
            JSON.stringify({
              kind: 'key',
              key: e.key,
              shift: e.shiftKey,
              target: (e.target as HTMLElement | null)?.className ?? String(e.target),
            }),
          ),
        );
      }

      // Triage. Single keys, so they yield to text fields like everything else.
      switch (e.key) {
        case 'e':
        case 'E':
          e.preventDefault();
          return a.triage('archive');
        case '#':
          e.preventDefault();
          return a.triage('trash');
        case '!':
          e.preventDefault();
          return a.triage('spam');
        case 's':
        case 'S':
          e.preventDefault();
          return a.toggleStar();
        case 'c':
        case 'C':
          e.preventDefault();
          return a.compose();
        case 'r':
        case 'R':
          e.preventDefault();
          return a.reply(false);
        case 'a':
        case 'A':
          e.preventDefault();
          return a.reply(true);
        case 'f':
        case 'F':
          e.preventDefault();
          return a.forward();
        case 'x':
        case 'X':
          e.preventDefault();
          return a.select();
        case 'J':
          if (e.shiftKey) {
            e.preventDefault();
            return a.extendSelection(true);
          }
          break;
        case 'K':
          if (e.shiftKey) {
            e.preventDefault();
            return a.extendSelection(false);
          }
          break;
        case '\\':
          e.preventDefault();
          return a.toggleReaderFull();
        case 'Escape':
          // Only meaningful when something is selected; dialogs handle their
          // own Escape and never reach here.
          return a.clearSelection();
        case 'b':
        case 'B':
          e.preventDefault();
          return a.snooze();
        case 'v':
        case 'V':
          e.preventDefault();
          return a.openMove();
        case 'l':
        case 'L':
          e.preventDefault();
          return a.openTag();
        case 'z':
        case 'Z':
          e.preventDefault();
          return a.undo();
        case 'I':
          if (e.shiftKey) {
            e.preventDefault();
            return a.triage('mark_read');
          }
          break;
        case 'U':
          if (e.shiftKey) {
            e.preventDefault();
            return a.triage('mark_unread');
          }
          break;
      }

      switch (e.key) {
        case 'g':
        case 'G':
          chord.current = { key: 'g', at: Date.now() };
          return;
        case 'Enter':
          if (listRow(e.target)) {
            e.preventDefault();
            return a.openConversation(true);
          }
          // The list's own Enter, only when the focus is not on something that
          // has an Enter of its own.
          if (activatable(e.target)) return;
          e.preventDefault();
          return a.openConversation(false);
        case 'u':
        case 'U':
          e.preventDefault();
          return a.backToList();
        // Plain letters, deliberately below the shifted cases above: ⇧I marks
        // read and ⇧U marks unread, and both return before reaching here. The
        // same split `u` and ⇧U already live under.
        case 'i':
          e.preventDefault();
          return a.moveToInbox();
        case 'o':
        case 'O':
          e.preventDefault();
          return a.popOut();
        case 'F6':
          e.preventDefault();
          return a.cyclePanes(e.shiftKey);
        case '/':
          e.preventDefault();
          return a.focusSearch();
        case '?':
          e.preventDefault();
          return a.openHelp();
      }
    };

    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
}
