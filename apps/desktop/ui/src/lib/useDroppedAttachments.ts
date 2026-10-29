import { useRef, type Dispatch, type SetStateAction } from 'react';
import type { Draft } from '../components/Compose';
import {
  copiedDraft,
  planDrop,
  stageDropped,
  withAttached,
  type Attached,
  type Copying,
} from './attachments';

type Job = {
  cancelled: boolean;
  promise: Promise<Attached>;
  /** Starts the byte copy. Idempotent, and a no-op read when already cancelled. */
  begin: () => void;
};

/**
 * Shows a dropped file's chip before the bytes have been copied, and finishes
 * the copy before a save or a send.
 *
 * The name and the size are known on the drop. The copy used to start in the
 * same turn, and its continuation runs before the browser paints, so the chip
 * was in the tree and still not on screen. The copy starts on a later turn.
 */
export function useDroppedAttachments(opts: {
  getDraft: () => Draft | null;
  setDraft: Dispatch<SetStateAction<Draft | null>>;
  /** The open message. A drop stays with the message that received it. */
  composer: () => unknown;
  stage: (name: string, bytes: Uint8Array) => Promise<Attached>;
  onRejected: (names: string[]) => void;
  onFailed: (error: unknown) => void;
}) {
  const jobs = useRef(new Map<string, Job>());
  // A copy that finished before the save asked for it. The job is gone by
  // then, and the draft the save was handed may still list the chip.
  const done = useRef(new Map<string, Attached>());
  const optsRef = useRef(opts);
  optsRef.current = opts;

  const drop = (files: readonly File[]) => {
    const current = optsRef.current.getDraft();
    if (!current) return;
    const slot = optsRef.current.composer();
    const reserved: Attached[] = [
      ...(current.attachments ?? []),
      ...(current.copying ?? []).map((c) => ({ path: c.id, name: c.name, size: c.size })),
    ];
    const plan = planDrop(
      files.map((file) => ({ name: file.name, size: file.size })),
      reserved,
    );
    if (plan.rejected.length > 0) optsRef.current.onRejected(plan.rejected);
    const added: Copying[] = [];
    const starting: Job[] = [];
    for (const index of plan.take) {
      const file = files[index];
      if (!file) continue;
      const id = crypto.randomUUID();
      const base = current.attachments ?? [];
      const job = deferredCopy(id, file, base, slot);
      jobs.current.set(id, job);
      starting.push(job);
      added.push({ id, name: file.name, size: file.size });
    }
    if (added.length === 0) return;
    // The chip first. The copy is a later turn, so this paint is not stuck
    // behind reading the file.
    optsRef.current.setDraft((cur) =>
      cur ? { ...cur, copying: [...(cur.copying ?? []), ...added] } : cur,
    );
    window.setTimeout(() => {
      for (const job of starting) {
        if (!job.cancelled) job.begin();
      }
    }, 0);
  };

  /** A copy that does not start until `begin`. Sending can start it early. */
  function deferredCopy(id: string, file: File, base: readonly Attached[], slot: unknown): Job {
    let started = false;
    const job: Job = {
      cancelled: false,
      promise: Promise.resolve({ path: '', name: file.name, size: file.size }),
      begin: () => {},
    };
    job.promise = new Promise<Attached>((resolve, reject) => {
      job.begin = () => {
        if (started) return;
        started = true;
        if (job.cancelled) {
          reject(new Error('cancelled'));
          return;
        }
        void stageDropped([file], base, optsRef.current.stage)
          .then((result) => {
            if (result.rejected.includes(file.name) || result.kept.length === base.length) {
              optsRef.current.onRejected([file.name]);
              throw new Error('rejected');
            }
            const staged = result.kept[result.kept.length - 1];
            if (!staged) throw new Error('not staged');
            return staged;
          })
          .then(resolve, reject);
      };
    });
    void job.promise.then(
      (ready) => {
        done.current.set(id, ready);
        jobs.current.delete(id);
        if (job.cancelled || optsRef.current.composer() !== slot) return;
        let refused: string[] = [];
        optsRef.current.setDraft((cur) => {
          if (!cur?.copying?.some((c) => c.id === id)) return cur;
          const placed = withAttached(cur.attachments ?? [], [ready]);
          refused = placed.rejected;
          return {
            ...cur,
            copying: cur.copying.filter((c) => c.id !== id),
            attachments: placed.kept,
          };
        });
        if (refused.length > 0) optsRef.current.onRejected(refused);
      },
      (error: unknown) => {
        jobs.current.delete(id);
        if (job.cancelled || optsRef.current.composer() !== slot) return;
        optsRef.current.setDraft((cur) =>
          cur ? { ...cur, copying: (cur.copying ?? []).filter((c) => c.id !== id) } : cur,
        );
        if (error instanceof Error && (error.message === 'rejected' || error.message === 'cancelled')) {
          return;
        }
        optsRef.current.onFailed(error);
      },
    );
    return job;
  }

  const remove = (id: string) => {
    const job = jobs.current.get(id);
    if (job) {
      job.cancelled = true;
      // Settle a copy that has not started, without reading the file.
      job.begin();
    }
    done.current.delete(id);
    jobs.current.delete(id);
    optsRef.current.setDraft((cur) =>
      cur ? { ...cur, copying: (cur.copying ?? []).filter((c) => c.id !== id) } : cur,
    );
  };

  /** The message with every chip it was showing either copied or dropped.
   *  Does not write into a composer that has since been replaced. */
  const finish = (draft: Draft, slot: unknown): Promise<Draft> =>
    copiedDraft(draft, async (id) => {
      const job = jobs.current.get(id);
      if (job?.cancelled) return null;
      const settled = done.current.get(id);
      if (settled) return settled;
      if (!job) return null;
      // A send does not wait out the paint gap. The chip is already up.
      job.begin();
      try {
        const ready = await job.promise;
        return job.cancelled ? null : ready;
      } catch {
        return null;
      }
    }).then((ready) => {
      for (const item of draft.copying ?? []) {
        done.current.delete(item.id);
        jobs.current.delete(item.id);
      }
      if (optsRef.current.composer() !== slot) return ready;
      const ids = new Set((draft.copying ?? []).map((c) => c.id));
      optsRef.current.setDraft((cur) => {
        if (!cur) return cur;
        const copying = (cur.copying ?? []).filter((c) => !ids.has(c.id));
        const landed = (ready.attachments ?? []).filter(
          (a) => !(cur.attachments ?? []).some((have) => have.path === a.path),
        );
        return {
          ...cur,
          copying,
          attachments: withAttached(cur.attachments ?? [], landed).kept,
        };
      });
      return ready;
    });

  return { drop, remove, finish };
}
