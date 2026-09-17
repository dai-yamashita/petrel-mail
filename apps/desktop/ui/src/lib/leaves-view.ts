import type { ActionKind } from './api';

/** The views that are about where a conversation is filed. Filing it
 *  somewhere else takes it out of these and only these. */
function isPlacementView(view: string): boolean {
  return (
    view === 'inbox' ||
    view === 'archive' ||
    view === 'sent' ||
    view === 'drafts' ||
    view === 'spam' ||
    view === 'trash' ||
    view.startsWith('folder:')
  );
}

/** Whether an action takes a conversation out of the list you are looking at.
 *
 *  This cannot be read off the action alone. Archiving removes a row from the
 *  inbox and from trash, but not from the archive; unstarring removes it from
 *  Starred and from nowhere else. Deciding by action alone left rows sitting in
 *  lists they no longer belonged to. */
export function leavesView(kind: ActionKind, view: string): boolean {
  // Gone entirely, so not here, wherever here is.
  if (kind === 'delete_forever') return true;

  // Filed somewhere specific. That changes where the conversation sits, and
  // only the views that are about where it sits lose it. Starred, Snoozed
  // and a tag are about a mark on the conversation, which filing does not
  // touch: this used to say "not here" for every view, and a starred
  // conversation dragged onto a folder from Starred vanished from the list
  // as if the star had gone with it. The store had kept it all along.
  if (kind === 'move') return isPlacementView(view);

  // Trash and spam are exclusive placements on both kinds of provider — the
  // conversation leaves wherever it was. So it leaves whatever list you happen
  // to be looking at, unless that list is where it lands.
  //
  // This used to be enumerated view by view, and the enumeration was wrong:
  // Sent, Drafts, Snoozed and every tag view were all listed as places nothing
  // moves out of, so binning something from any of them left the row sitting
  // there until a refresh took it away.
  if (kind === 'trash') return view !== 'trash';
  if (kind === 'spam') return view !== 'spam';

  // Out of the inbox, and out of a bin it is being rescued from. Stars and tags
  // survive archiving, so those views keep the conversation.
  if (kind === 'archive') return view === 'inbox' || view === 'trash' || view === 'spam';

  if (kind === 'snooze') return view === 'inbox';
  if (kind === 'unsnooze') return view === 'snoozed';
  if (kind === 'unstar') return view === 'starred';

  // Untagging is deliberately not here. The row only leaves if the tag removed
  // is the one being viewed, and this cannot see which tag was passed — so it
  // leaves the row alone rather than risk removing one the user is still
  // looking at. The next load has it right.
  return false;
}

/** Whether this view lists individual messages rather than conversations.
 *
 *  Drafts is the only one: a draft is a thing you finish, not a conversation.
 *  Everywhere else a row stands for a whole correspondence, and a verb aimed at
 *  it means the correspondence.
 *
 *  This is the distinction that made binning a leftover reply draft file the
 *  whole thread. Triage is a conversation verb and is handed the row's
 *  `thread_id`; once a draft has been pushed it shares its conversation's
 *  thread, so the draft's row named every live member. Their mail left the
 *  inbox, your own replies left Sent, and the toast said only "Moved to Trash".
 *  A draft that had never been pushed has no thread and filed only itself,
 *  which is why the path looked correct until somebody tidied up after
 *  answering something. */
export function listsPerMessage(view: string): boolean {
  return view === 'drafts';
}

/** Whether a verb can be aimed at one message rather than a conversation.
 *
 *  Where a message sits is a property of that message. Whether a conversation
 *  is read, starred, tagged or snoozed is a property of the conversation, and
 *  the store refuses those one row at a time rather than half-applying them —
 *  the read state in particular runs thread-wide and would leave undo restoring
 *  one flag of several. So in Drafts the placement verbs act on the draft and
 *  the marks still act on the conversation, which is also how a draft row
 *  behaves in Thunderbird and Apple Mail. */
export function actsOnOneMessage(kind: ActionKind): boolean {
  return kind === 'trash' || kind === 'spam' || kind === 'archive' || kind === 'move';
}
