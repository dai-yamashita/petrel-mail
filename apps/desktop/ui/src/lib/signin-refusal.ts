import { t } from './strings';

/**
 * The shell's refusal to reach the server for an account that cannot sign
 * in, in the person's words.
 *
 * Empty Trash, Move all to Trash, Mark all read, and renaming or deleting a
 * folder used to ask the server anyway, one sign-in per message for Empty
 * Trash, while the password was refused; an account with no password Petrel
 * could read had its Trash emptied here only, while the server kept it. They
 * answer in English now (`signin::SIGN_IN_FIRST` in the shell), and this puts
 * it in the person's language. Anything else is a real failure and keeps its
 * own wording.
 */
export function signinRefusal(error: unknown): string | null {
  return String(error).includes('sign in to this account again first')
    ? t('signin-first')
    : null;
}

/**
 * What became of a folder made here whose copy on the server is not there
 * yet. Signed out, the shell does not ask the server and says so; the folder
 * is kept here and goes with the sync after signing in again. Said as
 * "Created" once, for a folder the server never got.
 */
export function folderPending(name: string, error: unknown): string {
  return signinRefusal(error)
    ? t('folder-made-here-signin', { name })
    : t('folder-server-pending', { name, error: String(error) });
}
