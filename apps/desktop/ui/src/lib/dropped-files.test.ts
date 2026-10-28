import { describe, expect, it } from 'vitest';
import { filesFromDataTransfer, filesFromDropCapture } from './dropped-files';

function fileList(files: File[]): FileList {
  const list = {
    length: files.length,
    item: (index: number) => files[index] ?? null,
  };
  files.forEach((file, index) => {
    Object.defineProperty(list, index, { value: file, enumerable: true });
  });
  return list as FileList;
}

/** Just enough of a DataTransfer for drop handling to read. */
function dropTransfer(
  files: File[],
  items: {
    kind: string;
    type: string;
    getAsFile?: () => File | null;
  }[] = [],
): DataTransfer {
  return {
    files: fileList(files),
    items: items.map((entry) => ({
      kind: entry.kind,
      type: entry.type,
      getAsFile:
        entry.getAsFile ??
        (() => (entry.kind === 'file' ? new File([''], 'x', { type: entry.type }) : null)),
    })),
  } as unknown as DataTransfer;
}

describe('filesFromDataTransfer', () => {
  it('null is not a drop', () => {
    expect(filesFromDataTransfer(null)).toEqual([]);
  });

  it('uses files when present and does not read items', () => {
    const fromFiles = new File(['a'], 'from-files.png', { type: 'image/png' });
    const transfer = {
      files: fileList([fromFiles]),
      get items(): DataTransferItemList {
        throw new Error('items must not be consulted when files is non-empty');
      },
    } as unknown as DataTransfer;

    const result = filesFromDataTransfer(transfer);
    expect(result).toEqual([fromFiles]);
  });

  it('falls back to items when files is empty', () => {
    const pdf = new File(['%PDF'], 'doc.pdf', { type: 'application/pdf' });
    const png = new File(['png'], 'shot.png', { type: 'image/png' });
    const transfer = dropTransfer([], [
      { kind: 'file', type: 'application/pdf', getAsFile: () => pdf },
      { kind: 'file', type: 'image/png', getAsFile: () => png },
    ]);

    const result = filesFromDataTransfer(transfer);
    expect(result).toEqual([pdf, png]);
    expect(result[0]?.type).toBe('application/pdf');
  });

  it('skips non-file items and items whose getAsFile returns null', () => {
    const kept = new File(['ok'], 'kept.bin', { type: 'application/octet-stream' });
    const transfer = dropTransfer([], [
      { kind: 'string', type: 'text/plain' },
      { kind: 'file', type: 'application/pdf', getAsFile: () => null },
      { kind: 'file', type: 'application/octet-stream', getAsFile: () => kept },
    ]);

    expect(filesFromDataTransfer(transfer)).toEqual([kept]);
  });
});

describe('filesFromDropCapture', () => {
  it('does not read items when the file list is empty', () => {
    // WKWebView leaves `files` empty and puts the file on `items`. Calling
    // getAsFile() here, in the capture phase, consumes it. The editor, which
    // is the drop target on a reply body, then sees null and no chip appears.
    const pdf = new File(['%PDF'], 'doc.pdf', { type: 'application/pdf' });
    let reads = 0;
    const transfer = {
      files: fileList([]),
      get items(): DataTransferItemList {
        reads += 1;
        return [
          {
            kind: 'file',
            type: 'application/pdf',
            getAsFile: () => {
              reads += 1;
              return pdf;
            },
          },
        ] as unknown as DataTransferItemList;
      },
    } as unknown as DataTransfer;

    expect(filesFromDropCapture(transfer)).toEqual([]);
    expect(reads).toBe(0);
  });

  it('keeps the file list when the browser already filled it', () => {
    const pdf = new File(['%PDF'], 'doc.pdf', { type: 'application/pdf' });
    expect(filesFromDropCapture(dropTransfer([pdf]))).toEqual([pdf]);
  });

  it('leaves getAsFile for the drop target, which can call it only once', () => {
    const pdf = new File(['%PDF'], 'doc.pdf', { type: 'application/pdf' });
    let reads = 0;
    const transfer = dropTransfer([], [
      {
        kind: 'file',
        type: 'application/pdf',
        getAsFile: () => {
          reads += 1;
          return reads === 1 ? pdf : null;
        },
      },
    ]);

    expect(filesFromDropCapture(transfer)).toEqual([]);
    expect(reads).toBe(0);
    expect(filesFromDataTransfer(transfer)).toEqual([pdf]);
    expect(reads).toBe(1);
  });
});
