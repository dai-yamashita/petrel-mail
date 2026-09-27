import { t } from '../lib/strings';

/** The petrel: a European storm petrel in flight, in the logo's small-size cut.
 *  The full mark's eye and wing line would vanish at this size. Drawn inline
 *  rather than shipped as an asset so it inherits the accent colour and stays
 *  crisp at any scale. */
export function TitleBar({ synced }: { synced: string }) {
  return (
    <div className="titlebar">
      <div className="wordmark">
        <svg width="20" height="14" viewBox="0 0 44 30" role="img" aria-label={t('app-name')}>
          <path
            d="M39.3 21.8C38.6 21.7 38.2 21.9 37.5 21.8C36.9 21.8 36.4 22 35.6 21.9C35 22.3 33.9 23.3 33.4 23.9C32.3 25.1 30.7 26.2 28.9 27.2C26.6 28.6 25.4 28.9 22.3 29C19.2 29.1 17.9 27.9 15.3 27.7C11.8 27.3 7.7 25.9 4.7 25.2C9 24.3 15.3 22.1 17.5 21.7C16.8 18.3 13.5 12.9 11.8 9.8C10.4 9.5 9.5 8.2 10.2 7.4C8.5 6.6 7.2 4.7 7.8 3.7C6.3 3.3 5.4 1.9 6 1C8.6 1.7 19 5.9 22.3 8.1C25.7 10.2 24.9 16.1 27.2 19.1C27.7 19.8 28.4 18.1 29.4 17.5C30.7 16.8 30.9 15.9 32.9 16C34.9 16.1 35.4 17.7 36 18.9C36.2 19.2 36.7 19.4 37.1 19.5C37.7 19.7 38.3 20 38.9 20.3C39.3 20.5 39.1 21.4 39.3 21.8Z"
            fill="var(--accent)"
          />
        </svg>
        <span>{t('app-name')}</span>
      </div>
      <span className="sync">{synced}</span>
    </div>
  );
}
