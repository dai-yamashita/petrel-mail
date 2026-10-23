import { Dialog } from '@ariakit/react';
import { useEffect, useState } from 'react';
import { api } from '../lib/api';
import { SOURCE_URL } from '../lib/project';
import { t } from '../lib/strings';

/**
 * What Petrel is and which version this is, for Petrel › About Petrel.
 *
 * Windows and Linux only. macOS draws its own About panel from the bundle; the
 * box the menu library draws elsewhere wraps the same facts in English no
 * setting can change, so the words here come from the bundle like every other
 * string in the app.
 *
 * The version is read from the build, as Settings › Updates reads it: opening
 * this must not be an update check, which the Updates pane promises never
 * happens uninvited.
 */
export function AboutDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    if (!open || version !== null) return;
    let live = true;
    api
      .appVersion()
      .then((s) => live && setVersion(s.current))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [open, version]);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      className="confirm-backdrop"
      backdrop={<div className="palette-scrim" />}
      aria-label={t('menubar-about')}
    >
      <div className="confirm">
        <div className="confirm-title">{t('app-name')}</div>
        <p className="confirm-detail">
          {version ? t('update-running', { version }) : t('update-reading')}
        </p>
        <p className="confirm-detail">{t('about-tagline')}</p>
        <div className="confirm-foot">
          {/* Close first: it is where focus lands, and Enter on an About box
              should put it away, not open a browser. */}
          <button type="button" className="reply" onClick={onClose}>
            {t('close')}
          </button>
          <button type="button" className="reply" onClick={() => void api.openExternal(SOURCE_URL)}>
            {t('help-source')}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
