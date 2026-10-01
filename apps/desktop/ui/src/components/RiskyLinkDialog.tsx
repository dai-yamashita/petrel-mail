import { Dialog } from '@ariakit/react';
import type { HomographRisk } from '../lib/links';
import { t } from '../lib/strings';

/**
 * A link that reads as one address and resolves to another.
 *
 * Both spellings are shown, and the safe answer is the default: doing nothing
 * leaves the browser unopened. One component for every window that shows mail,
 * because the pop-out opened the same links with no question at all.
 */
export function RiskyLinkDialog({
  risky,
  onDismiss,
}: {
  risky: { risk: HomographRisk; open: () => void } | null;
  onDismiss: () => void;
}) {
  return (
    <Dialog
      open={risky !== null}
      onClose={onDismiss}
      className="confirm-backdrop"
      backdrop={<div className="palette-scrim" />}
      aria-label={t('link-risk-title')}
    >
      <div className="confirm" role="alertdialog">
        <div className="confirm-title">{t('link-risk-title')}</div>
        <p className="confirm-detail">
          {t('link-risk-body', {
            typed: risky?.risk.asTyped ?? '',
            real: risky?.risk.asPunycode ?? '',
          })}
        </p>
        <div className="confirm-foot">
          <button type="button" className="reply" onClick={onDismiss}>
            {t('link-risk-stay')}
          </button>
          <button
            type="button"
            className="reply danger"
            onClick={() => {
              risky?.open();
              onDismiss();
            }}
          >
            {t('link-risk-open')}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
