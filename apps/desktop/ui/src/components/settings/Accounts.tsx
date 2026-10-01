import { useEffect, useState } from 'react';
import { api, type Account } from '../../lib/api';
import { count as fmtCount, listTime } from '../../lib/format';
import { t } from '../../lib/strings';
import { Confirm } from '../Confirm';
import { SignInAgain } from '../SignInAgain';

const COLORS = ['#0E7C86', '#9A6B1F', '#6B7F87', '#3B6EA5', '#6B5CA5', '#5E7C4A'];
const ROLES = ['archive', 'sent', 'drafts', 'spam', 'trash'] as const;

export function Accounts({
  onAddAccount,
  onAccountRemoved,
  onMessage,
}: {
  onAddAccount: () => void;
  onAccountRemoved: (wasActive: boolean) => void;
  onMessage?: (text: string) => void;
}) {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [removing, setRemoving] = useState<Account | null>(null);
  const [signingIn, setSigningIn] = useState<Account | null>(null);
  // What "Export first…" in the remove confirmation came to, said in it —
  // and only in the confirmation of the account it was for. Kept as one
  // string, a slow export cancelled out of one account's dialog said its
  // result in the next account's.
  const [exported, setExported] = useState<{ account: number; text: string } | null>(null);
  // Whose export is being written. Remove waits for it: the export reads the
  // account's mail, which the removal deletes.
  const [exporting, setExporting] = useState<number | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () =>
    api
      .accounts()
      .then((a) => {
        setAccounts(a);
        setError(null);
        setSelected((cur) => (a.some((x) => x.id === cur) ? cur : (a[0]?.id ?? null)));
      })
      .catch((err: unknown) => setError(String(err)));

  useEffect(() => {
    void load();
  }, []);

  const account = accounts.find((a) => a.id === selected) ?? null;

  /** The account's mail, to a file the person picks, before it goes: what
   *  exists only on this computer is not on the server to come back.
   *
   *  Mail only. The export writes what Petrel holds a stored copy of, and a
   *  draft or a message waiting to send has none; the confirmation says so
   *  before the button is pressed, and the result says so again. */
  const exportFirst = async (a: Account) => {
    setExporting(a.id);
    try {
      const path = await api.pickSavePath(`petrel-${a.email}.mbox`, 'mbox');
      if (!path) return;
      const [written] = (await api.exportMbox(a.id, 'all', path)).split('/');
      setExported({
        account: a.id,
        // A number, so the sentence takes its plural ("1 messages" was the
        // raw text in every language) and its grouping.
        text: t('accounts-remove-exported', { count: Number(written), account: a.email }),
      });
    } catch (e) {
      setExported({ account: a.id, text: t('storage-export-failed', { error: String(e) }) });
    } finally {
      setExporting(null);
    }
  };

  if (error) {
    return (
      <div className="pane-body">
        <h1 className="pane-title">{t('settings-accounts')}</h1>
        <div className="empty">
          <h2 style={{ color: 'var(--danger)' }}>{t('accounts-failed')}</h2>
          <p className="mono" style={{ fontSize: 11.5 }}>{error}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-accounts')}</h1>

      <section className="field">
        <div className="field-head">
          <div className="flabel">{t('accounts-yours')}</div>
          {/* The same three steps a first run walks, in a dialog. */}
          <button type="button" className="reply" onClick={onAddAccount}>
            {t('accounts-add')}
          </button>
        </div>
        <div className="account-list">
          {accounts.map((a) => (
            <button
              key={a.id}
              type="button"
              className="account-row"
              aria-current={a.id === selected ? 'true' : undefined}
              onClick={() => setSelected(a.id)}
            >
              <span className="dot" style={{ background: a.color || 'var(--ink3)' }} />
              <span className="account-main">
                <span className="account-email clip">{a.email}</span>
                <span className="tiny">
                  {a.signin ? (
                    <span className="account-signin">{t('accounts-needs-signin')}</span>
                  ) : (
                    <>
                      {a.display_name || a.kind}
                      {a.newest_ms ? ` · ${t('accounts-synced', { when: listTime(a.newest_ms) })}` : ''}
                    </>
                  )}
                </span>
              </span>
              <span className="mono tiny">
                {a.unread_count > 0 ? fmtCount(a.unread_count) : '—'}
              </span>
            </button>
          ))}
          {accounts.length === 0 && <p className="fhelp">{t('accounts-none')}</p>}
        </div>
      </section>

      {account && (
        <>
          <section className="field">
            <div className="flabel">{account.email}</div>
            <p className="fhelp">
              {t('accounts-storage', { count: account.message_count })}
            </p>
            <div className="box">
              {/* For every account, not only one that has failed: a password
                  changed on purpose is entered here before the server starts
                  refusing the old one. */}
              <div className="row2">
                <div className="t">
                  <b>{t('accounts-signin')}</b>
                  <span className={account.signin ? 'account-signin' : undefined}>
                    {account.signin
                      ? t(account.signin === 'missing' ? 'signin-missing' : 'signin-refused', {
                          email: account.email,
                        })
                      : t('accounts-signin-help')}
                  </span>
                </div>
                <button type="button" className="reply" onClick={() => setSigningIn(account)}>
                  {t('signin-again')}
                </button>
              </div>

              <div className="row2">
                <div className="t">
                  <b>{t('accounts-colour')}</b>
                  <span>{t('accounts-colour-help')}</span>
                </div>
                <div className="dotrow">
                  {COLORS.map((c) => (
                    <button
                      key={c}
                      type="button"
                      className={`acc sm${account.color === c ? ' on' : ''}`}
                      style={{ background: c }}
                      aria-label={c}
                      aria-pressed={account.color === c}
                      onClick={() => {
                        api
                          .setAccountColor(account.id, c)
                          .then(() => {
                            void api.log(`set_account_color ok account=${account.id} ${c}`);
                            return load();
                          })
                          .catch((err: unknown) => {
                            // Never silent: a write that fails and a write that
                            // changes nothing visible look identical otherwise.
                            setError(String(err));
                            void api.log(`set_account_color FAILED: ${err}`);
                          });
                      }}
                    />
                  ))}
                </div>
              </div>

              <div className="row2">
                <div className="t">
                  <b>{t('accounts-keep')}</b>
                  {/* Q24 in one line: what happens here when the server forgets. */}
                  <span>
                    {account.local_archive ? t('accounts-keep-archive') : t('accounts-keep-mirror')}
                  </span>
                </div>
                <div className="pill">
                  <button
                    type="button"
                    className={!account.local_archive ? 'on' : undefined}
                    onClick={() => {
                      api
                        .setAccountArchive(account.id, false)
                        .then(load)
                        .catch((err: unknown) => setError(String(err)));
                    }}
                  >
                    {t('accounts-mirror')}
                  </button>
                  <button
                    type="button"
                    className={account.local_archive ? 'on' : undefined}
                    onClick={() => {
                      api
                        .setAccountArchive(account.id, true)
                        .then(load)
                        .catch((err: unknown) => setError(String(err)));
                    }}
                  >
                    {t('accounts-archive')}
                  </button>
                </div>
              </div>
            </div>
          </section>

          <section className="field">
            <div className="flabel">{t('accounts-folders')}</div>
            <p className="fhelp">{t('accounts-folders-help')}</p>
            {account.folders.length > 0 ? (
              <div className="folder-grid">
                {ROLES.map((role) => {
                  const f = account.folders.find((x) => x.role === role);
                  return (
                    <div className="folder-cell" key={role}>
                      <div className="tiny">{t(`folder-${role}` as never)}</div>
                      <div className="clip folder-path">{f?.path ?? t('folder-unmapped')}</div>
                    </div>
                  );
                })}
              </div>
            ) : (
              <p className="fhelp folder-none">{t('accounts-folders-none')}</p>
            )}
          </section>

          <section className="field last">
            <div className="flabel">{t('accounts-remove')}</div>
            <p className="fhelp">{t('accounts-remove-help')}</p>
            <button type="button" className="reply danger" onClick={() => setRemoving(account)}>
              {t('accounts-remove')}
            </button>
          </section>
        </>
      )}

      <SignInAgain
        account={signingIn}
        onClose={() => setSigningIn(null)}
        onSignedIn={(email) => {
          setSigningIn(null);
          onMessage?.(t('signin-done', { email }));
          void load();
        }}
      />

      <Confirm
        open={removing !== null}
        title={t('accounts-remove-confirm', { email: removing?.email ?? '' })}
        detail={t('accounts-remove-body')}
        confirmLabel={t('accounts-remove')}
        note={exported && exported.account === removing?.id ? exported.text : null}
        confirmDisabled={exporting !== null && exporting === removing?.id}
        extra={
          removing
            ? {
                label:
                  exporting === removing.id
                    ? t('accounts-remove-exporting')
                    : t('accounts-remove-export'),
                disabled: exporting !== null,
                onClick: () => void exportFirst(removing),
              }
            : null
        }
        onClose={() => {
          // This account's result goes with its dialog; another account's,
          // still on its way, stays for that account.
          const closing = removing?.id;
          setRemoving(null);
          setExported((e) => (e && e.account === closing ? null : e));
        }}
        onConfirm={() => {
          const a = removing;
          setRemoving(null);
          setExported((e) => (e && e.account === a?.id ? null : e));
          if (!a) return;
          void api
            .removeAccount(a.id)
            .then(() => {
              // Told before the list reloads: the window behind this pane
              // may be showing the account that just went.
              onAccountRemoved(a.active);
              return load();
            })
            .catch((e) => setError(String(e)));
        }}
      />
    </div>
  );
}
