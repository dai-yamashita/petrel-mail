import { describe, expect, it } from 'vitest';
import { ACTION_LABEL, actionLabel } from './bulk';
import { STRING_IDS } from './string-ids';
import { t } from './strings';

describe('the names actions go by in the bulk confirmation', () => {
  it('names every action from the bundle', () => {
    // A record over every kind, so a new one cannot reach the dialog without a
    // label; this checks each label is a real string and not an id on screen.
    for (const [kind, label] of Object.entries(ACTION_LABEL)) {
      expect(STRING_IDS, kind).toContain(label);
      expect(t(label), kind).not.toBe(label);
    }
  });

  it('calls a move back to the inbox that', () => {
    expect(actionLabel('move', true)).toBe('menu-move-inbox');
    expect(actionLabel('move', false)).toBe(ACTION_LABEL.move);
    expect(actionLabel('archive', true)).toBe(ACTION_LABEL.archive);
  });
});
