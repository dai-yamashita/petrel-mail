import { describe, expect, it } from 'vitest';
import { MENU_COMMANDS, choiceView, menuFor, type ChoiceValues, type MenuNode } from './menu';
import { REPO_URL } from './project';
import { sortKeys } from './sort';
import { STRING_IDS } from './string-ids';
import { t } from './strings';

/* A menu bar cannot be read back out of the DOM, and this environment grants
   neither Accessibility nor Screen Recording, so nothing can look at one. What
   is checkable is the description the menu is built from: that every item has
   a name people will actually see, that every command has somewhere to go, and
   that no two accelerators fight. Those are the failures that would otherwise
   only show up as a blank row or a dead key in a shipped build. */

function walk(nodes: MenuNode[], depth = 0): { node: MenuNode; depth: number }[] {
  return nodes.flatMap((node) =>
    node.role === 'submenu'
      ? [{ node, depth }, ...walk(node.items, depth + 1)]
      : [{ node, depth }],
  );
}

const MAC = menuFor(true);
const OTHER = menuFor(false);

const submenu = (menu: MenuNode[], label: string) => {
  const found = walk(menu).find(({ node }) => node.role === 'submenu' && node.label === label)?.node;
  return found?.role === 'submenu' ? found.items : [];
};
const natives = (items: MenuNode[]) =>
  items.map((i) => (i.role === 'native' ? i.native : null)).filter(Boolean);

describe.each([
  ['macOS', MAC],
  ['Windows and Linux', OTHER],
])('the menu bar on %s', (_platform, menu) => {
  const all = walk(menu);

  it('names every item from the bundle', () => {
    const labelled = all.filter(({ node }) => node.role !== 'separator');
    expect(labelled.length).toBeGreaterThan(0);
    for (const { node } of labelled) {
      if (node.role === 'separator') continue;
      expect(STRING_IDS).toContain(node.label);
      // t() falls back to the id when a string is missing. An id in a menu bar
      // is the exact failure the Fluent migration was supposed to end.
      expect(t(node.label)).not.toBe(node.label);
      expect(t(node.label).trim()).not.toBe('');
    }
  });

  it('sends every command somewhere', () => {
    const commands = all
      .map(({ node }) => (node.role === 'command' ? node.command : null))
      .filter((c): c is (typeof MENU_COMMANDS)[number] => c !== null);
    // The point of the check: an item that names a command nothing handles is
    // an item that silently does nothing when clicked.
    for (const command of commands) expect(MENU_COMMANDS).toContain(command);
    expect(commands.length).toBeGreaterThan(0);
  });

  it('gives no two items the same key', () => {
    const keys = all
      .map(({ node }) => (node.role === 'command' ? node.accelerator : undefined))
      .filter((a): a is string => Boolean(a));
    expect(new Set(keys).size).toBe(keys.length);
    // Modifier+key, in the form muda parses. A typo here is a menu item that
    // builds and then never responds to its own printed shortcut.
    for (const key of keys) expect(key).toMatch(/^(CmdOrCtrl|Shift|Alt)(\+(CmdOrCtrl|Shift|Alt))*\+.+$/);
  });

  it('carries the two shortcuts the plan names', () => {
    const commands = all.filter(({ node }) => node.role === 'command');
    const find = (command: string) =>
      commands.find(({ node }) => node.role === 'command' && node.command === command)?.node;
    const compose = find('new-message');
    const settings = find('settings');
    expect(compose && compose.role === 'command' && compose.accelerator).toBe('CmdOrCtrl+N');
    expect(settings && settings.role === 'command' && settings.accelerator).toBe('CmdOrCtrl+,');
  });

  it('keeps a full Edit menu, which is what dictation and the emoji picker need', () => {
    expect(natives(submenu(menu, 'menubar-edit'))).toEqual(['Undo', 'Redo', 'Cut', 'Copy', 'Paste', 'SelectAll']);
  });

  /* The one item in Edit that Petrel implements itself, and the reason the
     menu is worth opening. ⌘F searches the whole open conversation, across
     every message frame in it, and before this it existed only for people who
     already knew to press it. The natives around it work wherever there is
     text and cannot be discovered from the menu either way. */
  it('offers Find in Conversation, on the key macOS apps put it', () => {
    const find = submenu(menu, 'menubar-edit').find((i) => i.role === 'command' && i.command === 'find');
    expect(find, 'Find belongs in Edit, where every Mac app keeps it').toBeDefined();
    if (find?.role === 'command') {
      expect(find.accelerator).toBe('CmdOrCtrl+F');
      expect(find.label).toBe('menubar-find');
    }
  });

  it('points its links at the project, over https', () => {
    const links = all
      .map(({ node }) => (node.role === 'link' ? node.url : null))
      .filter((u): u is string => u !== null);
    expect(links.length).toBeGreaterThan(0);
    for (const url of links) {
      // A menu item is a click straight out of the app. http here would be a
      // downgrade nobody sees coming, and a host typo is a link that looks
      // right and lands somewhere else entirely.
      expect(url.startsWith(`${REPO_URL}/`) || url.startsWith(`${REPO_URL}#`)).toBe(true);
    }
    expect(new Set(links).size).toBe(links.length);
    expect(REPO_URL.startsWith('https://github.com/')).toBe(true);
  });

  it('gives the Help menu no key equivalent', () => {
    const help = menu.find((n) => n.role === 'submenu' && n.helpMenu);
    const items = help?.role === 'submenu' ? help.items : [];
    // ? opens the Help pane from the window. A menu accelerator would get
    // first refusal on that key and take it away from every text field, which
    // is the Toggle Full Screen lesson running in the other direction.
    for (const item of items) {
      if (item.role === 'command') expect(item.accelerator).toBeUndefined();
    }
    expect(items.some((i) => i.role === 'command' && i.command === 'help')).toBe(true);
  });

  it('offers every value each setting can take', () => {
    const values = (setting: string) =>
      all
        .map(({ node }) => (node.role === 'choice' && node.setting === setting ? node.value : null))
        .filter(Boolean);
    // A value added to settings.tsx and not to the menu is a menu that cannot
    // show the state the app is in.
    expect(values('theme')).toEqual(['light', 'dark', 'system']);
    expect(values('density')).toEqual(['relaxed', 'compact']);
    expect(values('layout')).toEqual(['right', 'below', 'off']);
    expect(values('railCollapsed')).toEqual(['off', 'on']);
    expect(values('checkboxes')).toEqual(['on', 'off']);
    // In the sort control's own order, so the two read the same.
    expect(values('sortKey')).toEqual(sortKeys(true));
    expect(values('sortDirection')).toEqual(['descending', 'ascending']);
  });

  it('steps the reading text on the keys browsers use for it', () => {
    const size = submenu(menu, 'menubar-text-size');
    const keyOf = (command: string) => {
      const item = size.find((i) => i.role === 'command' && i.command === command);
      return item?.role === 'command' ? item.accelerator : undefined;
    };
    expect(keyOf('text-bigger')).toBe('CmdOrCtrl+=');
    expect(keyOf('text-smaller')).toBe('CmdOrCtrl+-');
    expect(keyOf('text-reset')).toBe('CmdOrCtrl+0');
  });

  it('leaves no submenu empty', () => {
    for (const { node } of all) {
      if (node.role === 'submenu') expect(node.items.length).toBeGreaterThan(0);
    }
  });

  /* Items are left out per platform, and a separator that stood between two
     of them is easy to leave behind. AppKit's window list is the one thing
     that may follow a trailing separator. */
  it('draws no separator at an edge or next to another', () => {
    for (const { node } of all) {
      if (node.role !== 'submenu') continue;
      const roles = node.items.map((i) => i.role);
      for (let i = 1; i < roles.length; i++) {
        expect(roles[i] === 'separator' && roles[i - 1] === 'separator', node.label).toBe(false);
      }
      expect(roles[0], node.label).not.toBe('separator');
      if (!(node.windowMenu && menu === MAC)) expect(roles[roles.length - 1], node.label).not.toBe('separator');
    }
  });
});

describe('the macOS menu bar', () => {
  it('puts the app menu first and Help last, as macOS expects', () => {
    const titles = MAC.map((n) => (n.role === 'submenu' ? n.label : null));
    expect(titles[0]).toBe('app-name');
    expect(titles[titles.length - 1]).toBe('menubar-help');
    expect(titles[titles.length - 2]).toBe('menubar-window');
    // Only AppKit can draw the window list, and it draws it into whichever
    // submenu is handed to it. Exactly one, or the list lands nowhere. The
    // same holds for Help and its search box.
    const windows = MAC.filter((n) => n.role === 'submenu' && n.windowMenu);
    const helps = MAC.filter((n) => n.role === 'submenu' && n.helpMenu);
    expect(windows.length).toBe(1);
    expect(helps.length).toBe(1);
    // Two flags on one submenu would hand the same object to AppKit twice for
    // two different jobs, and the second claim wins silently.
    expect(MAC.some((n) => n.role === 'submenu' && n.windowMenu && n.helpMenu)).toBe(false);
  });

  it('keeps the full screen item the menu it replaced had', () => {
    // ⌃⌘F is not a system binding: it works only while some menu item claims
    // it. Replacing Tauri's default View menu without this took full screen
    // off the keyboard, which is a loss no new item makes up for.
    expect(natives(submenu(MAC, 'menubar-view'))).toContain('Fullscreen');
  });

  it('shows the standard About panel, and the items only AppKit has', () => {
    const app = submenu(MAC, 'app-name');
    expect(app.some((i) => i.role === 'about')).toBe(true);
    expect(natives(app)).toEqual(['Services', 'Hide', 'HideOthers', 'ShowAll', 'Quit']);
    const zoom = submenu(MAC, 'menubar-window').find((i) => i.role === 'native' && i.native === 'Maximize');
    expect(zoom?.role === 'native' && zoom.label).toBe('menubar-zoom');
  });
});

describe('the Windows and Linux menu bar', () => {
  /* muda draws every item it is handed and answers only what its platform
     has. These were rows that did nothing — and Hide hid the window from the
     taskbar too, with nothing to bring it back. */
  it('draws none of the items only AppKit answers', () => {
    const drawn = walk(OTHER).map(({ node }) => (node.role === 'native' ? node.native : null));
    for (const mac of ['Services', 'Hide', 'HideOthers', 'ShowAll', 'Fullscreen'] as const) {
      expect(drawn).not.toContain(mac);
    }
  });

  it('opens Petrel’s own About rather than the box muda draws', () => {
    const app = submenu(OTHER, 'app-name');
    expect(app.some((i) => i.role === 'about')).toBe(false);
    const about = app.find((i) => i.role === 'command' && i.command === 'about');
    expect(about?.role === 'command' && about.label).toBe('menubar-about');
  });

  it('calls the window’s largest size Maximize, as these platforms do', () => {
    const max = submenu(OTHER, 'menubar-window').find((i) => i.role === 'native' && i.native === 'Maximize');
    expect(max?.role === 'native' && max.label).toBe('menubar-maximize');
  });

  it('still lets you quit, from the menu that held it', () => {
    expect(natives(submenu(OTHER, 'app-name'))).toEqual(['Quit']);
  });
});

describe('every command the hook answers', () => {
  it('is reachable from the menu on some platform', () => {
    // A name in MENU_COMMANDS that no item uses is a handler for nothing.
    const used = new Set(
      [...walk(MAC), ...walk(OTHER)]
        .map(({ node }) => (node.role === 'command' ? node.command : null))
        .filter(Boolean),
    );
    for (const command of MENU_COMMANDS) expect(used).toContain(command);
  });
});

describe('how a View choice reads', () => {
  const values: ChoiceValues = {
    theme: 'system',
    density: 'relaxed',
    layout: 'right',
    railCollapsed: 'off',
    checkboxes: 'off',
    sortKey: 'date',
    sortDirection: 'descending',
  };
  const node = (setting: keyof ChoiceValues, value: string) => {
    const found = walk(OTHER).find(
      ({ node: n }) => n.role === 'choice' && n.setting === setting && n.value === value,
    )?.node;
    if (found?.role !== 'choice') throw new Error(`no ${setting}:${value} in the menu`);
    return found;
  };

  it('ticks what the setting holds, and only that', () => {
    expect(choiceView(node('layout', 'right'), values, false).checked).toBe(true);
    expect(choiceView(node('layout', 'below'), values, false).checked).toBe(false);
    expect(choiceView(node('checkboxes', 'off'), values, false).checked).toBe(true);
    expect(choiceView(node('railCollapsed', 'on'), values, false).checked).toBe(false);
  });

  it('offers Best match only while there is a search to rank', () => {
    // The control above the list leaves it out of a mailbox; a native menu
    // cannot leave an item out, so it greys it.
    expect(choiceView(node('sortKey', 'relevance'), values, false).enabled).toBe(false);
    expect(choiceView(node('sortKey', 'relevance'), values, true).enabled).toBe(true);
    expect(choiceView(node('sortKey', 'sender'), values, false).enabled).toBe(true);
  });

  it('names the directions in the words of the key in force', () => {
    expect(choiceView(node('sortDirection', 'descending'), values, false).text).toBe(t('sort-newest'));
    expect(choiceView(node('sortDirection', 'ascending'), values, false).text).toBe(t('sort-oldest'));
    const byName = { ...values, sortKey: 'sender' as const };
    expect(choiceView(node('sortDirection', 'descending'), byName, false).text).toBe(t('sort-z-a'));
    expect(choiceView(node('sortDirection', 'ascending'), byName, false).text).toBe(t('sort-a-z'));
  });

  it('claims no direction for a ranking, which has none', () => {
    const ranked = { ...values, sortKey: 'relevance' as const, sortDirection: '' as const };
    for (const way of ['descending', 'ascending']) {
      const view = choiceView(node('sortDirection', way), ranked, true);
      expect(view.checked).toBe(false);
      expect(view.enabled).toBe(false);
    }
    expect(choiceView(node('sortKey', 'relevance'), ranked, true).checked).toBe(true);
  });
});
