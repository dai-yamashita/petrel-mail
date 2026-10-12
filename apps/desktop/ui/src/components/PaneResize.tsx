import { followPress } from '../lib/press';
import { t } from '../lib/strings';

/**
 * The divider between the conversation list and the reading pane.
 *
 * A separate component rather than another strip inside one of the panes: it
 * belongs to neither, and putting it in either would make that pane responsible
 * for a width that is really the layout's.
 *
 * Given a `separator` role and a tab stop for the same reason the rail's is —
 * a layout you can only change by holding a mouse button down is a layout some
 * people cannot change at all.
 */
export function PaneResize({ onResize }: { onResize: (xOrDelta: number) => void }) {
  return (
    <div
      className="pane-resize"
      role="separator"
      aria-orientation="vertical"
      aria-label={t('pane-resize')}
      tabIndex={0}
      // A context click here began a resize that no release ever ended, so the
      // panes followed the pointer until the next click: see `followPress`.
      onPointerDown={(e) => followPress(e, onResize)}
      onKeyDown={(e) => {
        const step = e.shiftKey ? 32 : 8;
        if (e.key === 'ArrowLeft') {
          e.preventDefault();
          onResize(-step);
        } else if (e.key === 'ArrowRight') {
          e.preventDefault();
          onResize(step);
        }
      }}
    />
  );
}
