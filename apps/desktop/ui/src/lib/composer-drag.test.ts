import { describe, expect, it } from 'vitest';
import { classifyComposerDrag } from './composer-drag';

describe('classifyComposerDrag', () => {
  it('takes a uri-list dropped on the reply body as a file', () => {
    // WKWebView often omits "Files" while the pointer is over the quoted
    // body. That is still a file drop, and the overlay has to cover the
    // editor so ProseMirror does not insert the file as text.
    expect(classifyComposerDrag(['text/uri-list'], false)).toBe('file');
  });

  it('leaves a uri-list dropped on a text field to that field', () => {
    expect(classifyComposerDrag(['text/uri-list'], true)).toBe('field');
  });

  it('takes a file dragged over a text field as a file', () => {
    expect(classifyComposerDrag(['Files', 'text/plain'], true)).toBe('file');
  });

  it('lets a selection move inside the body', () => {
    expect(classifyComposerDrag(['text/plain', 'text/html'], false)).toBe('through');
  });

  it('takes an undescribed drag over the body as a file', () => {
    expect(classifyComposerDrag([], false)).toBe('file');
  });

  it('leaves plain text dropped on a field in that field', () => {
    expect(classifyComposerDrag(['text/plain'], true)).toBe('field');
  });

  it('takes a file the browser named as Files, including over the body', () => {
    expect(classifyComposerDrag(['Files'], false)).toBe('file');
  });

  it('does not cover a body selection that also carries a uri-list as text', () => {
    // A dragged link and an in-editor selection both name text/plain. Treating
    // every uri-list as a file would put the overlay over the selection and
    // the move would land on the overlay. A file the engine forgot to label
    // as Files still attaches: capture does not call getAsFile(), so the
    // editor's drop handler is the one call that returns the file.
    expect(classifyComposerDrag(['text/uri-list', 'text/plain', 'text/html'], false)).toBe('through');
  });
});
