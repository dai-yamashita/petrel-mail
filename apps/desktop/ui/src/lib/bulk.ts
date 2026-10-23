import type { ActionKind } from './api';
import type { StringId } from './strings';

/**
 * Each action in the words it is offered in — the conversation menu's own, so
 * the confirmation for an action on a whole view names it as the menu did.
 * A record over every kind, so a new one cannot reach that dialog nameless.
 */
export const ACTION_LABEL: Record<ActionKind, StringId> = {
  archive: 'reader-archive',
  trash: 'cmd-trash',
  spam: 'menu-spam',
  star: 'menu-star',
  unstar: 'menu-unstar',
  mark_read: 'reader-mark-read',
  mark_unread: 'reader-mark-unread',
  move: 'picker-folder-title',
  tag: 'picker-tag-title',
  untag: 'action-untag',
  snooze: 'reader-snooze',
  unsnooze: 'action-unsnooze',
  delete_forever: 'delete-forever',
};

/** The label for an action, with a move back to the inbox called that rather
 *  than a move to some folder. */
export function actionLabel(kind: ActionKind, toInbox: boolean): StringId {
  return kind === 'move' && toInbox ? 'menu-move-inbox' : ACTION_LABEL[kind];
}
