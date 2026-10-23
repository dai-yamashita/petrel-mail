import { Dialog } from '@ariakit/react';
import type { ActionKind } from '../lib/api';
import { count as fmtCount } from '../lib/format';
import { t, type StringId } from '../lib/strings';

/** An action on a whole view, waiting on its yes. */
export type BulkAsk = {
  kind: ActionKind;
  target?: number;
  /** The view's own key, which is what the engine walks. */
  view: string;
  /** The view as the rail names it, for the question. */
  viewName: string;
  /** How many conversations the view holds, loaded or not. */
  count: number;
  /** What the action is called where it was offered. */
  label: StringId;
};

/**
 * "Apply to all 12,345 conversations?" — asked before an action reaches every
 * conversation in a view rather than the rows on screen.
 *
 * Asked because there is no undo afterwards (see `triage_view`), and because
 * the number is the point: a list that looks like forty rows can be a mailbox
 * of thousands. The safe answer is the default, as it is for every question
 * this app asks before something it cannot take back; the action keeps the
 * name the menu gave it, so the button says what it will do.
 */
export function BulkConfirmDialog({
  ask,
  onCancel,
  onConfirm,
}: {
  ask: BulkAsk | null;
  onCancel: () => void;
  onConfirm: (ask: BulkAsk) => void;
}) {
  const title = ask ? t('bulk-confirm-title', { count: fmtCount(ask.count) }) : '';
  // Binning is the kind that changes what the mailbox holds; it reads as the
  // dangerous button the other questions use for the same thing.
  const binning = ask?.kind === 'trash' || ask?.kind === 'spam' || ask?.kind === 'delete_forever';
  return (
    <Dialog
      open={ask !== null}
      onClose={onCancel}
      className="confirm-backdrop"
      backdrop={<div className="palette-scrim" />}
      aria-label={title}
    >
      <div className="confirm" role="alertdialog">
        <div className="confirm-title">{title}</div>
        <p className="confirm-detail">
          {ask && t('bulk-confirm-body', { action: t(ask.label), view: ask.viewName })}
        </p>
        <div className="confirm-foot">
          <button type="button" className="reply" onClick={onCancel}>
            {t('cancel')}
          </button>
          <button
            type="button"
            className={binning ? 'reply danger' : 'reply primary'}
            onClick={() => ask && onConfirm(ask)}
          >
            {ask && t(ask.label)}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
