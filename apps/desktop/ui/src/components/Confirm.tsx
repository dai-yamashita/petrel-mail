import { useEffect, useRef } from 'react';
import { Dialog, DialogDismiss } from '@ariakit/react';
import { clickAway } from '../lib/dialog';
import { t } from '../lib/strings';

type Props = {
  open: boolean;
  title: string;
  /** What will actually happen, in the user's terms. Not a restatement of the
   *  title — if it says nothing the title did not, leave it out. */
  detail?: string | null;
  /** The verb, on the button. "Delete", not "OK": a button labelled OK makes
   *  people read the prose to find out what they are agreeing to. */
  confirmLabel: string;
  onConfirm: () => void;
  onClose: () => void;
  /** A safe thing to do first, on the footer's far side from the verb:
   *  "Export first…" before an account's local mail is deleted. */
  extra?: { label: string; onClick: () => void; disabled?: boolean } | null;
  /** What that safe thing came to, said under the detail. */
  note?: string | null;
  /** The verb held back while the safe thing runs: removing an account
   *  while its export is still being written would take the export's
   *  source away. */
  confirmDisabled?: boolean;
};

/**
 * The dialog that stands in front of something irreversible.
 *
 * Petrel confirms almost nothing — undo is the better answer nearly every time,
 * and a client that asks "are you sure" after every gesture trains people to
 * dismiss it without reading, which is worse than not asking. This exists for
 * the small set of actions that undo genuinely cannot cover.
 *
 * Focus lands on Cancel, not on the destructive button. Someone who hits Return
 * out of habit should get the safe outcome; the one who means it can Tab once
 * or click. For the same reason the destructive button is never the one Enter
 * finds by default.
 */
export function Confirm({
  open,
  title,
  detail,
  confirmLabel,
  onConfirm,
  onClose,
  extra,
  note,
  confirmDisabled,
}: Props) {
  const cancel = useRef<HTMLButtonElement>(null);

  // Ariakit keeps the dialog mounted and hidden, so this has to run on each
  // opening rather than on mount — otherwise focus is set once, for the first
  // thing ever confirmed, and never again.
  useEffect(() => {
    if (open) cancel.current?.focus();
  }, [open]);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      // Ariakit puts focus on the first thing in the dialog that takes it,
      // which since "Export first…" is that button, not Cancel; the effect
      // above then raced it. Named here, Cancel is where focus lands.
      initialFocus={cancel}
      className="confirm-backdrop"
      {...clickAway(onClose)}
      backdrop={<div className="palette-scrim" onClick={onClose} />}
      aria-label={title}
    >
      {/* The name goes on this node, not only on the Ariakit dialog around it:
          an assistive technology treats the innermost dialog role as the dialog,
          and that one was announced with no name at all. */}
      <div className="confirm" role="alertdialog" aria-label={title}>
        <div className="confirm-title">{title}</div>
        {detail && <p className="confirm-detail">{detail}</p>}
        {/* Mounted with the dialog, and never hidden: empty until there is
            something to say, when it takes no room (confirm.css). A polite
            region that appears along with its text is not reliably announced,
            WebKit least of all, and `hidden` kept this one out of the
            accessibility tree until its text came. */}
        <p className="confirm-detail confirm-note" role="status" aria-live="polite">
          {note || null}
        </p>
        <div className="confirm-foot">
          {extra && (
            <>
              {/* Held back with aria-disabled, not disabled: the button that
                  starts the export is the one with focus, and a disabled
                  button lets it fall to the page for as long as the export
                  runs. */}
              <button
                type="button"
                className="reply"
                onClick={extra.disabled ? undefined : extra.onClick}
                aria-disabled={extra.disabled || undefined}
              >
                {extra.label}
              </button>
              <span className="spacer" />
            </>
          )}
          <DialogDismiss ref={cancel} className="reply">
            {t('cancel')}
          </DialogDismiss>
          <button
            type="button"
            className="reply danger"
            onClick={confirmDisabled ? undefined : onConfirm}
            aria-disabled={confirmDisabled || undefined}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
