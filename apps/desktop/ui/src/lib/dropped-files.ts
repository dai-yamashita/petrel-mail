/**
 * Capture-phase read. The file list only, never `items`.
 *
 * On WKWebView `getAsFile()` returns the file once. A capture handler that
 * calls it spends that call before the drop target — the overlay, or the
 * reply body when the overlay is not up — and the attachment never appears.
 */
export function filesFromDropCapture(data: DataTransfer | null): File[] {
  if (!data) return [];
  return Array.from(data.files);
}

/**
 * Files dragged onto the composer, read at the drop target.
 *
 * Drop handling must read the payload in the same turn as the event: after
 * the handler returns, `getAsFile()` on `DataTransferItem` yields null even
 * though the drop already happened. Callers read it synchronously from the
 * drop event for that reason.
 *
 * When `dataTransfer.files` is populated, that list is authoritative and
 * `items` is left alone. On WKWebView, `files` is often empty while the
 * dropped file still sits on `items`. Every file kind is kept, not only
 * images: a PDF dropped onto the composer must attach like anything else.
 */
export function filesFromDataTransfer(data: DataTransfer | null): File[] {
  if (!data) return [];

  const fromFiles = Array.from(data.files);
  if (fromFiles.length > 0) return fromFiles;

  return Array.from(data.items)
    .filter((item) => item.kind === 'file')
    .map((item) => item.getAsFile())
    .filter((file): file is File => file !== null);
}
