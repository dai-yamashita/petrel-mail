/* The native menu bar.
 *
 * Built here rather than in Rust for two reasons. The labels come from the
 * Fluent bundle, which lives in the window — a menu built in the shell would
 * need every string shipped across the IPC seam or duplicated in Rust, and a
 * duplicated string is a string that will one day disagree with itself. And the
 * items run the *same functions* the keyboard and the palette run, as closures,
 * not as an event id that has to be matched back to a command at the other end.
 * A menu is a way to find a command; like the palette, it is not a second
 * implementation of one (see lib/commands.ts).
 *
 * What was here before: Tauri installs a default macOS menu when an app sets
 * none, so Petrel has never been menu-less — it has had Tauri's generic one,
 * with a File menu holding only Close and a View menu holding only Full Screen.
 * `install` logs what it replaced, so that claim can be checked rather than
 * believed.
 *
 * Two arrangements from one description (`menuFor`). macOS gets the menu a Mac
 * app is expected to have — the app menu first, Window last. Windows and Linux
 * get the same menus without the items only AppKit implements. muda draws every
 * item it is handed but answers only the ones its platform has, so on Windows
 * those rows were drawn and did nothing — and Hide did worse, which is why the
 * split exists rather than being left for later.
 */

import { useEffect, useRef } from 'react';
import type {
  CheckMenuItem,
  MenuItemOptions,
  PredefinedMenuItemOptions,
  Submenu,
  SubmenuOptions,
} from '@tauri-apps/api/menu';
import { api } from './api';
import { isMac } from './keys';
import { t, type StringId } from './strings';
import type { Settings } from './settings';
import { directionLabels, type SortKey } from './sort';
import { ISSUES_URL, SOURCE_URL } from './project';

/** The items macOS implements itself. Their accelerators, their behavior and
 *  their place in the responder chain come with them: an Edit menu of these is
 *  what makes dictation and the emoji picker work inside a webview. */
type NativeItem = Extract<PredefinedMenuItemOptions['item'], string>;

/** A command Petrel answers itself. Adding a name here forces the hook below
 *  to provide a handler for it — the compiler, not a code review, is what
 *  stops a menu item that does nothing. */
export const MENU_COMMANDS = [
  'new-message',
  'settings',
  'help',
  'find',
  'about',
  'text-bigger',
  'text-smaller',
  'text-reset',
] as const;
export type MenuCommand = (typeof MENU_COMMANDS)[number];

/** Which way a sort runs. Named as the setting holds it; the words people see
 *  are the key's own (`directionLabels`). */
export type SortDirection = 'descending' | 'ascending';

/** The settings the View menu drives. Every one already has a home — Settings
 *  › Appearance and › Lists, the rail's collapse button, the sort control above
 *  the list — and the menu is another way to reach it, not another copy. The
 *  sort is two choices because that control is two: which key, which way. */
export type Choice =
  | { setting: 'theme'; value: Settings['theme'] }
  | { setting: 'density'; value: Settings['density'] }
  | { setting: 'layout'; value: Settings['layout'] }
  | { setting: 'railCollapsed'; value: Settings['railCollapsed'] }
  | { setting: 'checkboxes'; value: Settings['checkboxes'] }
  | { setting: 'sortKey'; value: SortKey }
  | { setting: 'sortDirection'; value: SortDirection };

/** What each choice reads as now. The direction is '' under Best match, which
 *  has none: it is the order the ranking produced. */
export type ChoiceValues = {
  theme: Settings['theme'];
  density: Settings['density'];
  layout: Settings['layout'];
  railCollapsed: Settings['railCollapsed'];
  checkboxes: Settings['checkboxes'];
  sortKey: SortKey;
  sortDirection: SortDirection | '';
};

export type MenuNode =
  | { role: 'separator' }
  | { role: 'native'; native: NativeItem; label: StringId }
  | { role: 'about'; label: StringId }
  | { role: 'command'; command: MenuCommand; label: StringId; accelerator?: string }
  | ({ role: 'choice'; label: StringId } & Choice)
  /** Opens an address in the browser. Not a command: there is no state to
   *  change and nothing for the window to do, so routing it through the
   *  command table would add a hop that carries nothing. */
  | { role: 'link'; label: StringId; url: string }
  | {
      role: 'submenu';
      label: StringId;
      items: MenuNode[];
      windowMenu?: true;
      helpMenu?: true;
    };

type ChoiceNode = Extract<MenuNode, { role: 'choice' }>;

/**
 * How one choice should read, given the state the window is in.
 *
 * Pure, so its rules can be asserted rather than looked at: a menu bar cannot
 * be read back out of the OS (menu.test.ts). The sort's rules are the list's
 * sort control's, so the two places that show the order cannot disagree.
 */
export function choiceView(
  node: { label: StringId } & Choice,
  values: ChoiceValues,
  searching: boolean,
): { text: string; checked: boolean; enabled: boolean } {
  const checked = values[node.setting] === node.value;
  if (node.setting === 'sortKey') {
    // Best match is an order only a search has; the control above the list
    // leaves it out of a mailbox, and a menu that cannot leave items out
    // greys it instead.
    return { text: t(node.label), checked, enabled: node.value !== 'relevance' || searching };
  }
  if (node.setting === 'sortDirection') {
    // In the key's own words, as the control says them: newest first for a
    // date, A to Z for a name. A ranking has no direction, so under Best match
    // both stand unticked and greyed rather than claiming one.
    const ranked = values.sortKey === 'relevance';
    const words = directionLabels(ranked ? 'date' : values.sortKey);
    return { text: t(words[node.value]), checked: !ranked && checked, enabled: !ranked };
  }
  return { text: t(node.label), checked, enabled: true };
}

/** The state a choice leaves behind, before the settings have caught up. */
function withChoice(values: ChoiceValues, choice: Choice): ChoiceValues {
  const next = { ...values, [choice.setting]: choice.value } as ChoiceValues;
  if (choice.setting === 'sortKey' && choice.value === 'relevance') next.sortDirection = '';
  return next;
}

/**
 * The menu, as data, for the platform it is drawn on.
 *
 * Kept declarative so it can be asserted on: menu.test.ts walks both
 * arrangements and checks that every label exists in the bundle, every command
 * has a handler, and no two accelerators collide. None of that is observable
 * from a running menu bar without a person looking at one.
 *
 * Away from macOS the app menu loses Services, Hide, Hide Others and Show All,
 * and View loses Toggle Full Screen. muda answers none of them on Windows bar
 * Hide, and Hide there is ShowWindow(SW_HIDE): the window leaves the screen and
 * the taskbar both, with no tray icon to call it back and no second launch to
 * hand it to — a running app with no way into it short of ending it. About
 * becomes Petrel's own, because the box muda draws in its place wraps the
 * values in English that no setting can change ("Name:", "Version:") in an
 * app that speaks eight languages.
 */
export function menuFor(mac: boolean): MenuNode[] {
  return [
    {
      role: 'submenu',
      label: 'app-name',
      items: [
        mac
          ? { role: 'about', label: 'menubar-about' }
          : { role: 'command', command: 'about', label: 'menubar-about' },
        { role: 'separator' },
        // The same pane ⌘, has always opened. The accelerator now belongs to
        // the menu: macOS gives its key equivalents first refusal, so the
        // window's own ⌘, handler no longer sees the keystroke. Both lead to
        // one place.
        { role: 'command', command: 'settings', label: 'menubar-settings', accelerator: 'CmdOrCtrl+,' },
        { role: 'separator' },
        ...(mac
          ? ([
              { role: 'native', native: 'Services', label: 'menubar-services' },
              { role: 'separator' },
              { role: 'native', native: 'Hide', label: 'menubar-hide' },
              { role: 'native', native: 'HideOthers', label: 'menubar-hide-others' },
              { role: 'native', native: 'ShowAll', label: 'menubar-show-all' },
              { role: 'separator' },
            ] satisfies MenuNode[])
          : []),
        { role: 'native', native: 'Quit', label: 'menubar-quit' },
      ],
    },
    {
      role: 'submenu',
      label: 'menubar-file',
      items: [
        { role: 'command', command: 'new-message', label: 'menubar-new-message', accelerator: 'CmdOrCtrl+N' },
        { role: 'separator' },
        { role: 'native', native: 'CloseWindow', label: 'menubar-close-window' },
      ],
    },
    {
      // Load-bearing beyond the obvious. Without these, macOS has nowhere to
      // send the editing commands dictation and the emoji picker issue, and
      // both misbehave inside the webview.
      //
      // They look inert more often than they are. Every one carries the real
      // AppKit selector with no target, so it travels the responder chain and
      // works wherever this app has text — the composer, the search field, a
      // folder being renamed, a selection in the reader — and does nothing
      // elsewhere. What it will not do is *say* so: the menu library calls
      // setAutoenablesItems(false) on every menu it builds, which is exactly
      // the macOS mechanism that would grey them out when nothing can respond.
      // So they stay black and clickable whether or not they apply. Reaching
      // past Tauri into the NSMenu to turn validation back on would couple
      // this file to another library's internals for the sake of some dimming.
      //
      // Find is the one item here Petrel implements itself, and it is the
      // reason this menu is worth opening: ⌘F searches the whole open
      // conversation, across every message frame in it, and until now existed
      // only for people who already knew to press it.
      role: 'submenu',
      label: 'menubar-edit',
      items: [
        { role: 'native', native: 'Undo', label: 'menubar-undo' },
        { role: 'native', native: 'Redo', label: 'menubar-redo' },
        { role: 'separator' },
        { role: 'native', native: 'Cut', label: 'menubar-cut' },
        { role: 'native', native: 'Copy', label: 'menubar-copy' },
        { role: 'native', native: 'Paste', label: 'menubar-paste' },
        { role: 'native', native: 'SelectAll', label: 'menubar-select-all' },
        { role: 'separator' },
        { role: 'command', command: 'find', label: 'menubar-find', accelerator: 'CmdOrCtrl+F' },
      ],
    },
    {
      // How the window looks and how the list is laid out, which is what a
      // View menu is for: everything here is a setting with a home elsewhere,
      // reachable from the menu bar too and ticked as it currently stands.
      role: 'submenu',
      label: 'menubar-view',
      items: [
        {
          role: 'submenu',
          label: 'appearance-theme',
          items: [
            { role: 'choice', setting: 'theme', value: 'light', label: 'theme-light' },
            { role: 'choice', setting: 'theme', value: 'dark', label: 'theme-dark' },
            { role: 'choice', setting: 'theme', value: 'system', label: 'theme-system' },
          ],
        },
        {
          role: 'submenu',
          label: 'appearance-density',
          items: [
            { role: 'choice', setting: 'density', value: 'relaxed', label: 'density-relaxed' },
            { role: 'choice', setting: 'density', value: 'compact', label: 'density-compact' },
          ],
        },
        {
          role: 'submenu',
          label: 'menubar-reading-pane',
          items: [
            { role: 'choice', setting: 'layout', value: 'right', label: 'layout-right' },
            { role: 'choice', setting: 'layout', value: 'below', label: 'layout-below' },
            { role: 'choice', setting: 'layout', value: 'off', label: 'layout-off' },
          ],
        },
        {
          role: 'submenu',
          label: 'settings-sidebar',
          items: [
            { role: 'choice', setting: 'railCollapsed', value: 'off', label: 'sidebar-expanded' },
            { role: 'choice', setting: 'railCollapsed', value: 'on', label: 'sidebar-collapsed' },
          ],
        },
        {
          role: 'submenu',
          label: 'appearance-checkboxes',
          items: [
            { role: 'choice', setting: 'checkboxes', value: 'on', label: 'checkboxes-on' },
            { role: 'choice', setting: 'checkboxes', value: 'off', label: 'checkboxes-off' },
          ],
        },
        { role: 'separator' },
        {
          // The same choices as the control above the list, in the same order,
          // applied to the same list — this mailbox's order or the search's.
          // Every key is drawn, because a native menu cannot add one when a
          // search starts; Best match is greyed until there is a query.
          role: 'submenu',
          label: 'menubar-sort-by',
          items: [
            { role: 'choice', setting: 'sortKey', value: 'relevance', label: 'sort-relevance' },
            { role: 'choice', setting: 'sortKey', value: 'date', label: 'sort-date' },
            { role: 'choice', setting: 'sortKey', value: 'sender', label: 'sort-sender' },
            { role: 'choice', setting: 'sortKey', value: 'subject', label: 'sort-subject' },
            { role: 'separator' },
            // Labelled for a date here; `choiceView` renames them for the key
            // in force, so a name reads A to Z rather than oldest first.
            { role: 'choice', setting: 'sortDirection', value: 'descending', label: 'sort-newest' },
            { role: 'choice', setting: 'sortDirection', value: 'ascending', label: 'sort-oldest' },
          ],
        },
        {
          // The keys browsers and reading apps use for the size of what is
          // being read, held to the slider's range in Settings › Appearance.
          role: 'submenu',
          label: 'menubar-text-size',
          items: [
            { role: 'command', command: 'text-bigger', label: 'menubar-text-bigger', accelerator: 'CmdOrCtrl+=' },
            { role: 'command', command: 'text-smaller', label: 'menubar-text-smaller', accelerator: 'CmdOrCtrl+-' },
            { role: 'command', command: 'text-reset', label: 'reset', accelerator: 'CmdOrCtrl+0' },
          ],
        },
        // Kept, not added. Tauri's default View menu held this one item, and
        // ⌃⌘F exists only because a menu item claims it — replacing the menu
        // without it would have quietly taken full screen off the keyboard.
        // macOS only: muda has no full screen to give it anywhere else, and a
        // row that does nothing is worse than no row.
        ...(mac
          ? ([
              { role: 'separator' },
              { role: 'native', native: 'Fullscreen', label: 'menubar-fullscreen' },
            ] satisfies MenuNode[])
          : []),
      ],
    },
    {
      // `windowMenu` hands this one to AppKit, which appends the list of open
      // windows to it. That list is the whole reason the menu is worth having
      // with a popped-out composer or message on screen, and it is not
      // something this file can draw for itself. Elsewhere nothing appends to
      // it, so the separator that would part the list from the items goes too.
      role: 'submenu',
      label: 'menubar-window',
      windowMenu: true,
      items: [
        { role: 'native', native: 'Minimize', label: 'menubar-minimize' },
        // Zoom is the Mac's word for it; everywhere else it is Maximize.
        { role: 'native', native: 'Maximize', label: mac ? 'menubar-zoom' : 'menubar-maximize' },
        ...(mac ? ([{ role: 'separator' }] satisfies MenuNode[]) : []),
      ],
    },
    {
      // Last, which is where macOS expects it, and marked so AppKit takes it:
      // a Help menu it owns gets the search box that walks the menus for you.
      // Tauri's default menu had an empty Help submenu, so replacing that menu
      // took the search box away; this puts it back and gives it something to
      // hold.
      role: 'submenu',
      label: 'menubar-help',
      helpMenu: true,
      items: [
        // No accelerator, deliberately. ? already opens this pane, and a menu
        // item claiming that key would take it from every text field in the
        // app — menus get first refusal on their key equivalents, which is the
        // same mechanism that nearly cost Toggle Full Screen, running the
        // other way.
        { role: 'command', command: 'help', label: 'menubar-petrel-help' },
        { role: 'separator' },
        { role: 'link', label: 'menubar-report-issue', url: ISSUES_URL },
        { role: 'link', label: 'menubar-view-source', url: SOURCE_URL },
      ],
    },
  ];
}

/** What the window has to supply for the menu to mean anything. The choices are
 *  read as well as written: a checkmark that does not follow the pane that owns
 *  the setting is worse than no checkmark. */
export type MenuBindings = {
  /** The same draft the C key and the palette open. */
  newMessage: () => void;
  /** The same pane ⌘, opens. */
  openSettings: () => void;
  /** The same pane ? opens. */
  openHelp: () => void;
  /** Petrel's own About, where the OS has no panel of its own to show. */
  openAbout: () => void;
  /** The same bar ⌘F opens, with the same rule about when there is anything
   *  to find in. */
  find: () => void;
  /** The reading text a step bigger or smaller, or back to the default. */
  textSize: (step: -1 | 0 | 1) => void;
  /** Where every View choice stands now. */
  values: ChoiceValues;
  /** Whether a search is running: Best match is an order only a search has. */
  searching: boolean;
  /** Applies a View choice as its own control would. */
  choose: (choice: Choice) => void;
};

type Built = CheckMenuItem | Submenu | MenuItemOptions | SubmenuOptions | PredefinedMenuItemOptions;

type Tauri = typeof import('@tauri-apps/api/menu');

/** A choice as drawn: the item in the menu bar and what it stands for. */
type ChoiceItem = { item: CheckMenuItem; node: ChoiceNode };

type BuildContext = {
  tauri: Tauri;
  /** Current values, so the checkmarks start honest. */
  state: ChoiceValues;
  searching: boolean;
  run: (command: MenuCommand) => void;
  choose: (choice: Choice) => void;
  /** Every choice item, so its mark, its greying and — for the sort's
   *  direction — its words can be corrected without rebuilding the menu. */
  checks: ChoiceItem[];
  /** The submenu AppKit should own, once the menu is installed. */
  windowMenu: Submenu | null;
  /** Likewise, the one it should treat as Help. */
  helpMenu: Submenu | null;
};

async function build(node: MenuNode, ctx: BuildContext): Promise<Built> {
  switch (node.role) {
    case 'separator':
      return { item: 'Separator' };
    case 'native':
      return { item: node.native, text: t(node.label) };
    case 'about':
      // Null metadata means the standard macOS panel, which reads the name,
      // version and icon out of the bundle. Passing our own would be a second
      // place for the version to live and a second place for it to be wrong.
      return { item: { About: null }, text: t(node.label) };
    case 'command':
      return {
        text: t(node.label),
        accelerator: node.accelerator,
        action: () => ctx.run(node.command),
      };
    case 'link':
      return {
        text: t(node.label),
        action: () => {
          void api.log(JSON.stringify({ kind: 'menu', command: `open:${node.url}` }));
          void api.openExternal(node.url);
        },
      };
    case 'choice': {
      const view = choiceView(node, ctx.state, ctx.searching);
      const item = await ctx.tauri.CheckMenuItem.new({
        text: view.text,
        checked: view.checked,
        enabled: view.enabled,
        action: () => ctx.choose(node),
      });
      ctx.checks.push({ item, node });
      return item;
    }
    case 'submenu': {
      const items: Built[] = [];
      for (const child of node.items) items.push(await build(child, ctx));
      if (!node.windowMenu && !node.helpMenu) return { text: t(node.label), items };
      // AppKit needs the real object, not a description of one.
      const submenu = await ctx.tauri.Submenu.new({ text: t(node.label), items });
      if (node.windowMenu) ctx.windowMenu = submenu;
      else ctx.helpMenu = submenu;
      return submenu;
    }
  }
}

/** Brings every choice item into line with where the settings stand. Only the
 *  sort's direction ever changes its words without the language changing,
 *  and a change of language rebuilds the whole menu. */
function refresh(checks: ChoiceItem[], values: ChoiceValues, searching: boolean): void {
  for (const { item, node } of checks) {
    const view = choiceView(node, values, searching);
    void item.setChecked(view.checked);
    void item.setEnabled(view.enabled);
    if (node.setting === 'sortDirection') void item.setText(view.text);
  }
}

/** The submenu titles of a menu, and how many items each holds, read back out
 *  of the OS rather than assumed from what we asked for. A menu bar is as
 *  opaque from outside as a webview is (AGENTS.md), and this is the only view
 *  into one that does not need a person looking at the screen. */
async function describe(menu: { items: () => Promise<unknown[]> }): Promise<string[]> {
  const out: string[] = [];
  for (const item of await menu.items()) {
    const submenu = item as { text?: () => Promise<string>; items?: () => Promise<unknown[]> };
    if (!submenu.text) continue;
    const title = await submenu.text();
    const count = submenu.items ? (await submenu.items()).length : 0;
    out.push(`${title}(${count})`);
  }
  return out;
}

/**
 * Builds the menu and hands it to the OS.
 *
 * Returns the choice items so their marks can be kept in step, and logs both
 * the menu it built and the menu it replaced.
 */
async function install(ctx: BuildContext): Promise<ChoiceItem[]> {
  const items: Built[] = [];
  for (const node of menuFor(isMac)) items.push(await build(node, ctx));
  const menu = await ctx.tauri.Menu.new({ items });
  const previous = await menu.setAsAppMenu();
  // Told to AppKit after the menu is in place; before, and there is no menu
  // bar for it to attach the window list to.
  await ctx.windowMenu?.setAsWindowsMenuForNSApp();
  // Without this macOS falls back to matching the *localized* word "Help",
  // which is exactly the guess that stops working in the six languages this
  // app now speaks. Saying which submenu it is does not depend on the title.
  await ctx.helpMenu?.setAsHelpMenuForNSApp();
  const replaced = previous ? await describe(previous) : [];
  void api.log(JSON.stringify({ kind: 'menu-installed', menu: await describe(menu), replaced }));
  // The old menu is nobody's now. Left alive it holds a resource per item for
  // the life of the process, and the language picker rebuilds this menu every
  // time it is used.
  if (previous) await previous.close();
  return ctx.checks;
}

/**
 * Puts Petrel's menu bar up, and keeps its checkmarks true.
 *
 * Built once. The handlers dispatch through a ref, so they always call the
 * current render's functions rather than the ones that existed at mount —
 * which is what lets the menu be installed once instead of on every keystroke
 * that changes the app's state.
 */
export function useAppMenu(bindings: MenuBindings): void {
  const ref = useRef(bindings);
  ref.current = bindings;
  const checks = useRef<ChoiceItem[] | null>(null);

  useEffect(() => {
    // Not under Tauri: `npm run dev` in a browser, and the harness, both run
    // this bundle with no menu API behind it. Nothing to do, and nothing worth
    // saying about it.
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    let live = true;

    void (async () => {
      try {
        const tauri = await import('@tauri-apps/api/menu');
        if (!live) return;
        const ctx: BuildContext = {
          tauri,
          state: ref.current.values,
          searching: ref.current.searching,
          run: (command) => {
            // Logged like the keyboard's own commands are, because a menu
            // click leaves no other trace: nothing changes in the DOM to say
            // where an action came from.
            void api.log(JSON.stringify({ kind: 'menu', command }));
            // A switch rather than an if/else chain so the compiler is the
            // thing that notices a new MENU_COMMAND with nowhere to go. The
            // chain this replaced ended in `else openSettings()`, which meant
            // any command it did not know about quietly opened Settings —
            // the exact silent-wrong-action the header claims cannot happen.
            switch (command) {
              case 'new-message':
                return ref.current.newMessage();
              case 'settings':
                return ref.current.openSettings();
              case 'help':
                return ref.current.openHelp();
              case 'find':
                return ref.current.find();
              case 'about':
                return ref.current.openAbout();
              case 'text-bigger':
                return ref.current.textSize(1);
              case 'text-smaller':
                return ref.current.textSize(-1);
              case 'text-reset':
                return ref.current.textSize(0);
              default:
                return ((x: never) => x)(command);
            }
          },
          choose: (choice) => {
            void api.log(JSON.stringify({ kind: 'menu', command: `${choice.setting}:${choice.value}` }));
            ref.current.choose(choice);
            // Corrected here as well as in the effect below: a check item
            // toggles itself on click, so choosing the value that was already
            // set would otherwise clear its own mark, and the effect will not
            // fire because nothing changed.
            refresh(checks.current ?? [], withChoice(ref.current.values, choice), ref.current.searching);
          },
          checks: [],
          windowMenu: null,
          helpMenu: null,
        };
        const built = await install(ctx);
        if (!live) return;
        checks.current = built;
        // Building takes a round trip per item, and a setting that moved in
        // the meantime was marked against the state the build started from.
        refresh(built, ref.current.values, ref.current.searching);
      } catch (e) {
        // A window with no menu bar is a poorer window, not a broken one. This
        // must never be the reason the app fails to start.
        void api.log(JSON.stringify({ kind: 'menu-failed', error: String(e) }));
      }
    })();

    return () => {
      live = false;
    };
  }, []);

  // Settings change from their own panes and controls too, and the marks
  // follow. Keyed on the values themselves rather than the object, which is a
  // new one every render: once per change, not once per keystroke.
  const stamp = JSON.stringify([bindings.values, bindings.searching]);
  useEffect(() => {
    const list = checks.current;
    if (!list) return;
    refresh(list, ref.current.values, ref.current.searching);
  }, [stamp]);
}
