import { useEffect, useRef, useState } from 'react';
import { AlertTriangle, Clock, Paperclip, WifiOff } from 'lucide-react';
import { api, type OutboxRow, type SignIn } from '../lib/api';
import { Icon } from './Icon';
import { t } from '../lib/strings';
import { outboxRefusal } from '../lib/outbox-refusal';
import { signinRefusal } from '../lib/signin-refusal';

/**
 * Every message Petrel is holding, and why.
 *
 * Not a list of conversations. Each row here is a message in one of five
 * states, and the row's job is to say which state in the words a person would
 * use and to offer exactly the actions that state allows. Four of the five
 * resolve themselves; the amber one cannot, and no amount of engineering makes
 * it — so it states what is unknown and hands over a choice.
 *
 * Re-read every second while open. The rows carry countdowns, and the worker
 * changes their state underneath; a view of the outbox that was true a minute
 * ago is the wrong thing to make decisions from.
 */

/** "7s", "2 min", "an hour" — the granularity a countdown is read at. */
function until(ms: number, now: number): string {
  const s = Math.max(0, Math.round((ms - now) / 1000));
  if (s < 60) return t('outbox-in-seconds', { count: s });
  const m = Math.round(s / 60);
  if (m < 60) return t('outbox-in-minutes', { count: m });
  return t('outbox-in-hours', { count: Math.round(m / 60) });
}

/** A scheduled time, as the row shows it: today's show a clock, others a date. */
function at(ms: number): string {
  const d = new Date(ms);
  const sameDay = new Date().toDateString() === d.toDateString();
  return sameDay
    ? d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
    : d.toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' });
}

/** The user-facing half of a 5xx: the code and the server's words. */
function reason(error: string | null): string {
  return (error ?? '').replace(/\s+/g, ' ').trim() || '—';
}

function Row({
  row,
  now,
  signin,
  onSignIn,
  onChange,
  onDiscard,
  onEdit,
  onRefused,
}: {
  row: OutboxRow;
  now: number;
  /** Set while the account cannot sign in: nothing goes until it can. */
  signin?: SignIn | null;
  onSignIn?: () => void;
  onChange: () => void;
  onDiscard: (row: OutboxRow) => void;
  /** `open` is the draft to resume: a new one when the message was deleted
   *  forever while it waited. */
  onEdit?: (id: number, open: number) => void;
  onRefused?: (text: string) => void;
}) {
  // One pull-back per row. The row stays up until the next redraw, and a
  // second press opened the message a second time.
  const pulling = useRef(false);
  const edit = () => {
    if (pulling.current) return;
    pulling.current = true;
    void api
      .outboxEdit(row.id)
      .then((open) => {
        onEdit?.(row.id, open ?? row.id);
        onChange();
      })
      .catch((e) => {
        pulling.current = false;
        // Too late to pull back: said, as Z and the bar say it, rather than
        // the row simply redrawing as "Sending…".
        const text = outboxRefusal(e);
        if (text) onRefused?.(text);
        onChange();
      });
  };
  const [checking, setChecking] = useState<string | null>(null);
  const act = (p: Promise<unknown>) => void p.then(onChange).catch(onChange);

  // A message inside its send-time window, still pullable back, as against
  // one waiting out a retry. Both are `RetryQueued` to the store; they read
  // very differently to a person.
  const pending = row.state === 'UndoWindow' || (row.state === 'RetryQueued' && row.attempts === 0);
  const waiting = row.state === 'RetryQueued' && row.attempts > 0;
  // Nothing reached the wire: the network is not there to reach.
  const offline = waiting && /unreachable|offline|no route|dns|resolve/i.test(row.error ?? '');
  const rejected = row.state === 'FailedPermanent';
  const unknown = row.state === 'NeedsAttention';
  const noSuchUser = rejected && /^5[0-9]{2}.*(no such user|unknown user|user unknown|does not exist)/i.test(row.error ?? '');

  // Signed out, a message that would go waits for the password instead, and
  // says so: the send worker holds it, as Thunderbird holds unsent mail until
  // you sign in. "Sending in 0s" over a message going nowhere said otherwise.
  const held = !!signin && (pending || waiting);
  // A message scheduled for later keeps its time while held: "Waiting for you
  // to sign in again" alone lost "Going at Tue 09:00".
  const scheduled = pending && row.send_after_ms - now >= 60_000;
  // Nor can Sent be looked in for one whose outcome is unknown: Check again
  // was a sign-in with the refused password, answered "cannot reach the
  // server". The row says why, and offers the sign-in instead.
  const checkWaits = unknown && !!signin;

  const tone = unknown ? 'amber' : rejected ? 'red' : waiting ? 'muted' : 'normal';

  return (
    <article className="outbox-row" data-tone={tone}>
      <header className="outbox-head">
        {unknown && <Icon icon={AlertTriangle} size={14} className="outbox-glyph" />}
        {offline && <Icon icon={WifiOff} size={14} className="outbox-glyph" />}
        {pending && <Icon icon={Clock} size={14} className="outbox-glyph" />}
        <span className="outbox-subject clip">
          {unknown && <span className="outbox-needs">{t('outbox-needs-you')} — </span>}
          {row.subject || t('no-subject')}
        </span>
      </header>
      <div className="outbox-to clip">
        {t('outbox-to', { who: row.to || '—' })}
        {row.attachments > 0 && (
          <>
            {' · '}
            <Icon icon={Paperclip} size={11} />{' '}
            {t('outbox-attachments', {
              count: row.attachments,
            })}
          </>
        )}
      </div>

      <p className="outbox-why">
        {row.state === 'Transmitting' && t('outbox-transmitting')}
        {held &&
          (scheduled
            ? t('signin-outbox-scheduled', { when: at(row.send_after_ms) })
            : t('signin-outbox-waiting'))}
        {pending &&
          !held &&
          (row.send_after_ms - now < 60_000
            ? t('outbox-sending-in', { when: until(row.send_after_ms, now) })
            : t('outbox-scheduled-for', { when: at(row.send_after_ms) }))}
        {waiting && !held && offline && t('outbox-offline')}
        {waiting && !held && !offline && t('outbox-retrying', { when: until(row.next_ms ?? now, now) })}
        {rejected &&
          t(noSuchUser ? 'outbox-rejected-user' : 'outbox-rejected', { reason: reason(row.error) })}
        {unknown && (
          <>
            {t('outbox-unknown-1')}
            <br />
            {t('outbox-unknown-2')}
            {checkWaits && (
              <>
                <br />
                {t('signin-outbox-check-waits')}
              </>
            )}
          </>
        )}
        {checking && <span className="outbox-checked"> {checking}</span>}
      </p>

      <div className="outbox-acts">
        {held && onSignIn && (
          <button type="button" className="reply primary" onClick={onSignIn}>
            {t('signin-again')}
          </button>
        )}
        {pending && (
          <>
            {/* Z is the bar's only while it counts; held, it is the last
                action's again (docs/24 #9), and the key is not offered. */}
            <button type="button" className="reply" onClick={edit}>
              {t('outbox-undo')}
              {!held && (
                <>
                  {' '}
                  <span className="kbd">Z</span>
                </>
              )}
            </button>
            {!held && (
              <button type="button" className="reply primary" onClick={() => act(api.outboxSendNow(row.id))}>
                {t('outbox-send-now')}
              </button>
            )}
          </>
        )}
        {waiting && !held && !offline && (
          <button type="button" className="reply primary" onClick={() => act(api.outboxSendNow(row.id))}>
            {t('outbox-try-now')}
          </button>
        )}
        {(waiting || rejected) && (
          <>
            <button type="button" className="reply" onClick={edit}>
              {t('outbox-edit')}
            </button>
            <button type="button" className="reply danger" onClick={() => onDiscard(row)}>
              {t('outbox-discard')}
            </button>
          </>
        )}
        {checkWaits && onSignIn && (
          <button type="button" className="reply primary" onClick={onSignIn}>
            {t('signin-again')}
          </button>
        )}
        {unknown && (
          <>
            {!checkWaits && (
              <button
                type="button"
                className="reply primary"
                onClick={() =>
                  void api
                    .outboxCheck(row.id)
                    .then((s) => {
                      setChecking(
                        s === 'Sent'
                          ? t('outbox-checked-sent')
                          : s === 'RetryQueued'
                            ? t('outbox-checked-absent')
                            : t('outbox-checked-unknown'),
                      );
                      onChange();
                    })
                    .catch((e) => setChecking(signinRefusal(e) ?? String(e)))
                }
              >
                {t('outbox-check-again')}
              </button>
            )}
            <button type="button" className="reply" onClick={() => act(api.outboxSendNow(row.id))}>
              {t('outbox-send-anyway')}
            </button>
            <button type="button" className="reply danger" onClick={() => onDiscard(row)}>
              {t('outbox-discard')}
            </button>
          </>
        )}
      </div>
    </article>
  );
}

export function Outbox({
  signin,
  onSignIn,
  onDiscard,
  onCountChange,
  onEdit,
  onRefused,
}: {
  /** Why the account cannot sign in, if it cannot: its unsent mail waits. */
  signin?: SignIn | null;
  onSignIn?: () => void;
  onDiscard: (row: OutboxRow) => void;
  /** Told how many need a person, so the rail can turn amber. */
  onCountChange?: (total: number, needsAttention: number) => void;
  /** After the send is pulled back: open the composer on `open`, the draft
   *  the pull-back handed over. */
  onEdit?: (id: number, open: number) => void;
  /** Undo or Edit came too late: the words to say so. */
  onRefused?: (text: string) => void;
}) {
  const [rows, setRows] = useState<OutboxRow[]>([]);
  const [now, setNow] = useState(() => Date.now());
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    api
      .outbox()
      .then((r) => {
        if (!live) return;
        setRows(r);
        onCountChange?.(r.length, r.filter((x) => x.state === 'NeedsAttention').length);
      })
      .catch((e) => api.log(`list_outbox failed: ${e}`));
    return () => {
      live = false;
    };
    // Re-read on every tick: the worker moves rows between states underneath.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tick]);

  useEffect(() => {
    const h = setInterval(() => {
      setNow(Date.now());
      setTick((n) => n + 1);
    }, 1000);
    return () => clearInterval(h);
  }, []);

  if (rows.length === 0) return null;

  return (
    <section className="outbox" aria-label={t('outbox-title')}>
      <h2 className="outbox-title">{t('outbox-title')}</h2>
      {rows.map((r) => (
        <Row
          key={r.id}
          row={r}
          now={now}
          signin={signin}
          onSignIn={onSignIn}
          onChange={() => setTick((n) => n + 1)}
          onDiscard={onDiscard}
          onEdit={onEdit}
          onRefused={onRefused}
        />
      ))}
    </section>
  );
}
