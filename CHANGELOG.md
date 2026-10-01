# Changelog

## 1.0.0

The first release.

Petrel is a desktop email client for macOS. It talks to your mail server over IMAP and
SMTP. Your mail is stored on your own machine, in SQLite and ordinary files. Nothing
goes through a service in the middle.

### Mail

- IMAP and SMTP. As many accounts as you want.
- Type your email address and Petrel works out the server settings. It knows 18
  providers by name, and falls back to looking them up.
- Gmail and iCloud work with app passwords.
- Servers that list their Sent and Junk folders without marking them as such still
  get them used as Sent and Spam, so sent copies are filed and reporting spam has
  somewhere to go.
- When a password stops working, Petrel asks you to sign in again rather than
  retrying every few minutes. An account's password and server settings can be
  changed in Settings.
- Messages are grouped into conversations. On Gmail it uses Gmail's own conversation
  ids, so the grouping matches what you see in the web app.
- New mail appears as it arrives. Petrel does not wait for a timer.
- An All Mail mailbox lists every conversation outside Spam and Trash, as Gmail's
  does.

### Reading

- Message HTML is cleaned up and then shown in a sandbox. No scripts run. It has no
  network access.
- Remote images and tracking pixels are blocked until you allow them. You can allow
  one message or one sender.
- Attachments preview, save and open. You get a warning before opening anything that
  can run.
- Images sent inside a message show inline.
- In dark mode, mail that only ships a light design gets darkened. Photos are left
  alone. Any message can be switched back to light.
- Meeting invitations show as a card with Accept, Tentative and Decline on it. Your
  answer is emailed back to the organiser, or, if you choose "Don't send a response",
  recorded without emailing anyone.
- Links open in your browser. Petrel warns you first if a web address uses characters
  that make it look like a different site.
- Newsletters that offer one-click unsubscribe can be left with one click.
- See a message's source, save it as an `.eml` file, or print it.
- Open a conversation in its own window.
- Japanese, Korean and Chinese mail in the older encodings (ISO-2022, HZ and the
  legacy code pages) reads correctly.

### Writing

- Rich text editor. Sends HTML with a plain text copy alongside.
- Reply, reply to all, and forward, with the quoting people expect. Replies go where
  the author asked, when a message names a Reply-To address.
- Replies keep the original's pictures, and forwards carry its attachments.
- To, Cc and Bcc. Bcc recipients get the message without anyone seeing them on it,
  and your own copy notes whom you blind-copied.
- A signature for each account. Changing who the mail is from changes the signature.
- Drafts save as you type, on your machine and on the server. A draft can be written
  in its own window.
- Undo send. You choose how long the pause is, up to 30 seconds.
- Send later: pick a time, and the message waits in the Outbox until then.
- The Outbox lists everything waiting to go, with Undo, Edit and Send now.
- A send that a crash interrupts is held for you to look at when Petrel comes back,
  never silently lost and never left stuck.
- If you write "attached" and there is no attachment, Petrel says so before sending.
- Addresses complete as you type, based on who you write to most and most recently.

### Organising

- Search your mail instantly. It understands terms like
  `from:alice has:attachment before:2026-01-01`, and AND, OR, NOT and brackets.
  The words a search matched are highlighted in the message.
- Save a search, and it stays in the sidebar.
- Large mailboxes stay quick. The list keeps a short window of rows and pages as you
  scroll, so an inbox of tens of thousands of conversations opens at once and
  scrolling still reaches the oldest thread.
- Archive, move, star, delete, mark as spam. Every one of them can be undone for ten
  seconds.
- Snooze a conversation, and it comes back to your Inbox at the time you pick.
- Make, rename, move and delete folders. They change on the server too, so webmail
  and your other devices see the same folders.
- Tags. On Gmail they become labels. On other servers they become IMAP keywords.
  Either way they sync both directions.
- Arrange the sidebar: drag folders and tags into the order you want, reorder its
  sections, and hide any but Mailboxes.
- Rules that file mail as it arrives.
- The Trash can empty itself after 7, 30 or 90 days. This is off unless you turn it
  on. The clock starts when a message reaches the Trash, not when it was sent, so
  binning an old message does not delete it straight away.

### The rest

- Keyboard shortcuts for everything, with a list you can search. Press ⌘K to run any
  command by name.
- Notifications for new mail in every account, with a pause button for 30 minutes,
  an hour, or until tomorrow. It silences Petrel only.
- Import mbox files and `.eml` files. Export any folder to mbox.
- Updates are signed, and only install when you ask. Petrel never checks on its own.
- Works with a keyboard alone. Every control is labelled for a screen reader.
- Eight languages: English, German, Spanish, French, Japanese, Korean, Brazilian
  Portuguese and Simplified Chinese. Petrel follows your system language, and you
  can pick another in Settings.

### Thanks

- dai-yamashita, whose Japanese account found four things before release that two
  ASCII accounts never would have: a reply that crashed the assembler, a Cc line
  that vanished, a send that sat in the outbox, and a list that loaded the whole
  mailbox.

### What is missing

Worth knowing before you install:

- macOS only.
- Outlook.com and Microsoft 365 mailboxes need Microsoft's own sign-in, which Petrel
  does not have yet. They cannot be added in this version.
- Gmail and Microsoft 365 are reached over IMAP, not their own APIs. This works, but
  Gmail prefers its API and may tighten IMAP access later.
- Notifications have no buttons on them yet.

### Requirements

macOS 11 or later with Safari 16.2 or later installed, on Apple silicon or Intel.
