import { Check, Minus } from 'lucide-react';
import type { Settings } from '../lib/settings';
import { coverage } from '../lib/selection';
import { t } from '../lib/strings';
import { Icon } from './Icon';

/**
 * The checkbox column's own header: every conversation the list holds, or
 * none. Drawn only while the column is, over the rows' boxes rather than
 * beside the title, so it reads as the column's and not the mailbox's.
 *
 * "Every" is the rows the list has loaded. A selection only ever acts on rows a
 * person can scroll to, and a row that loads afterwards arrives unticked, so
 * the box turns to a dash rather than claiming more than it took.
 *
 * A dash ticks the rest, as a partly-ticked box does in the pattern people know
 * it from; a full box clears. Escape still clears from anywhere.
 *
 * A button, unlike the rows' boxes: those sit in a list the arrow keys and X
 * already walk, and this is the one control here a keyboard can only reach by
 * Tab.
 */
export function SelectAllCheck({
  rows,
  selected,
  density,
  onSelectAll,
  onClear,
}: {
  rows: readonly { id: number }[];
  selected: ReadonlySet<number>;
  density: Settings['density'];
  onSelectAll: () => void;
  onClear: () => void;
}) {
  const covered = coverage(selected, rows);
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={covered === 'all' ? true : covered === 'some' ? 'mixed' : false}
      aria-label={t('list-select-all')}
      title={t('list-select-all')}
      className="row-check view-check"
      data-density={density}
      disabled={rows.length === 0}
      onClick={() => (covered === 'all' ? onClear() : onSelectAll())}
    >
      {covered === 'all' && <Icon icon={Check} size={11} />}
      {covered === 'some' && <Icon icon={Minus} size={11} />}
    </button>
  );
}
