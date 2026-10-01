import { useEffect, useState } from 'react';
import { api } from './api';

/**
 * What clicking a link inside a message does.
 *
 * A message body is a sandboxed frame with no navigation of its own: it catches
 * the click and posts the destination out, and this decides what opening it
 * means. Keeping that decision here rather than in the frame is the point — the
 * frame renders sender-controlled markup, so it is the last place that should
 * be trusted to say what a link is.
 *
 * Web links go to the browser, because a mail client is not one: rendering a
 * linked page inside the reading pane would put a live, sender-chosen document
 * where the user expects their mail, and the pane's whole defence is that it
 * never does that.
 *
 * `mailto:` stays in Petrel. Handing it to the system would open whichever
 * other mail program the machine happens to prefer, which is a strange answer
 * from the mail program you are already reading in.
 */
export type Link =
  | { kind: 'web'; url: string }
  | { kind: 'mail'; addr: string }
  | { kind: 'blocked' };

/**
 * A link whose visible spelling and real destination differ.
 *
 * The attack this exists for: `аpple.com` typed with a Cyrillic а reads as
 * `apple.com` to a person and resolves somewhere else entirely. Browsers
 * defuse it by showing the punycode form; a mail client hands the URL to
 * the browser, so the moment to say something is before it goes.
 */
export type HomographRisk = {
  /** What the address looks like: the destination's own letters. */
  asTyped: string;
  /** What it actually resolves to, in the ASCII form DNS uses. */
  asPunycode: string;
  reason: 'mixed-script' | 'latin-lookalike';
};

/** Letters of other alphabets that pass for Latin ones at a glance. The
 *  Cyrillic ones are the set Chromium's IDN spoof checker uses for the same
 *  question; the Greek and Armenian ones carry the same attack. Not the whole
 *  of UTS #39, only the letters that do the disguising. */
const LOOKS_LATIN = new Set([
  ...'\u0430\u0441\u0501\u0435\u04bb\u0456\u0458\u04cf\u043e\u0440\u051b\u0455\u051d\u0445\u0443\u044a\u044c\u04bd\u043f\u0433\u0475\u0461',
  ...'\u03bf\u03b1\u03bd\u03c1',
  ...'\u0561',
]);

/** Letters of the Latin script itself that stand in for plain ASCII ones:
 *  a script g or a small-capital o in an otherwise ordinary name. */
const LATIN_STAND_INS = /[\u0261\u1d0f]/u;

/**
 * What is wrong with one label, if anything.
 *
 * Judged a label at a time, as browsers do. A Cyrillic name under `.com` is
 * not mixing scripts, since its letters are all Cyrillic, and reading the
 * whole host at once used to question every such domain. Nor is a Cyrillic
 * letter that happens to look Latin a disguise on its own: the Cyrillic word
 * for Yandex has several and is plainly Cyrillic. The disguise is a label
 * whose every letter could pass for Latin, since only then does the whole
 * word read as a Latin one.
 */
function labelRisk(label: string): HomographRisk['reason'] | null {
  const letters = [...label].filter((c) => /\p{L}/u.test(c));
  if (letters.length === 0) return null;
  const latin = letters.some((c) => /\p{Script=Latin}/u.test(c));
  const other = letters.some((c) =>
    /[\p{Script=Cyrillic}\p{Script=Greek}\p{Script=Armenian}]/u.test(c),
  );
  if (latin && other) return 'mixed-script';
  if (LATIN_STAND_INS.test(label)) return 'latin-lookalike';
  if (other && letters.every((c) => LOOKS_LATIN.has(c))) return 'latin-lookalike';
  return null;
}

/** RFC 3492's constants, for decoding a punycode label. */
const PUNY = { base: 36, tMin: 1, tMax: 26, skew: 38, damp: 700, bias: 72, n: 128 } as const;
const MAX_INT = 0x7fffffff;

function punyDigit(code: number): number {
  if (code >= 0x30 && code <= 0x39) return code - 22; // 0-9 are 26-35
  if (code >= 0x41 && code <= 0x5a) return code - 0x41; // A-Z
  if (code >= 0x61 && code <= 0x7a) return code - 0x61; // a-z
  return PUNY.base;
}

function punyAdapt(delta: number, points: number, first: boolean): number {
  let d = first ? Math.floor(delta / PUNY.damp) : delta >> 1;
  d += Math.floor(d / points);
  let k = 0;
  while (d > ((PUNY.base - PUNY.tMin) * PUNY.tMax) >> 1) {
    d = Math.floor(d / (PUNY.base - PUNY.tMin));
    k += PUNY.base;
  }
  return k + Math.floor(((PUNY.base - PUNY.tMin + 1) * d) / (d + PUNY.skew));
}

/** One label's letters, from the part after `xn--`; null when it is not valid
 *  punycode. The decoding RFC 3492 gives, as browsers and Node do it. */
function decodeLabel(input: string): string | null {
  const out: number[] = [];
  let basic = input.lastIndexOf('-');
  if (basic < 0) basic = 0;
  for (let j = 0; j < basic; j += 1) {
    const code = input.charCodeAt(j);
    if (code >= 0x80) return null;
    out.push(code);
  }
  let n: number = PUNY.n;
  let bias: number = PUNY.bias;
  let i = 0;
  for (let at = basic > 0 ? basic + 1 : 0; at < input.length; ) {
    const before = i;
    let w = 1;
    for (let k = PUNY.base; ; k += PUNY.base) {
      if (at >= input.length) return null;
      const digit = punyDigit(input.charCodeAt(at++));
      if (digit >= PUNY.base || digit > Math.floor((MAX_INT - i) / w)) return null;
      i += digit * w;
      const t = k <= bias ? PUNY.tMin : k >= bias + PUNY.tMax ? PUNY.tMax : k - bias;
      if (digit < t) break;
      if (w > Math.floor(MAX_INT / (PUNY.base - t))) return null;
      w *= PUNY.base - t;
    }
    const length = out.length + 1;
    bias = punyAdapt(i - before, length, before === 0);
    if (Math.floor(i / length) > MAX_INT - n) return null;
    n += Math.floor(i / length);
    i %= length;
    out.splice(i, 0, n);
    i += 1;
  }
  if (out.length === 0) return null;
  try {
    return String.fromCodePoint(...out);
  } catch {
    return null;
  }
}

/**
 * A host's letters, decoded from the ASCII form DNS uses.
 *
 * The question a homograph raises is about where a link goes, so it is read
 * from the destination itself rather than from how the link happened to be
 * spelled. That matters more than it sounds: the message frame hands over the
 * anchor's `href`, which the browser has already turned into punycode, so a
 * check that read the sender's spelling was only ever shown `xn--pple-…` and
 * never saw the Cyrillic а it exists to catch. A label that will not decode
 * is left as it is.
 */
export function unicodeHost(ascii: string): string {
  return ascii
    .split('.')
    .map((label) =>
      label.toLowerCase().startsWith('xn--') ? (decodeLabel(label.slice(4)) ?? label) : label,
    )
    .join('.');
}

/**
 * Whether this link's spelling is worth a question first.
 *
 * Null for the ordinary case, including honest international domains: a
 * hostname written entirely in one non-Latin script is somebody's real
 * address, and warning about it would be warning about the existence of
 * other languages. What earns a question is a name that *borrows* — Latin
 * mixed with another script, or another script's letters chosen because
 * they look Latin.
 */
export function homographRisk(href: string): HomographRisk | null {
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return null;
  }
  const asPunycode = url.hostname;
  // No punycode label means no non-ASCII: nothing can be disguised.
  if (!asPunycode.split('.').some((label) => label.startsWith('xn--'))) return null;

  const asTyped = unicodeHost(asPunycode);
  // The last label is the top-level domain: a registry's fixed list rather
  // than anything a sender can pick, and a Cyrillic one such as рус is made
  // only of letters that look Latin. Judging it would question every address
  // under it. A bare single-label host is judged as it is.
  const labels = asTyped.split('.');
  const reasons = (labels.length > 1 ? labels.slice(0, -1) : labels).map(labelRisk);
  const reason = reasons.includes('mixed-script')
    ? 'mixed-script'
    : reasons.find((r) => r !== null);
  return reason ? { asTyped, asPunycode, reason } : null;
}

/**
 * Reads a link's destination.
 *
 * An allowlist, not a blocklist. `file:` reaches local content, `javascript:`
 * executes, and the custom schemes other applications register are a wide and
 * unaudited surface for a stranger to aim at — so anything unrecognised is
 * simply not a link we open.
 */
export function classifyLink(href: string): Link {
  const raw = href.trim();
  const scheme = raw.slice(0, raw.indexOf(':') + 1).toLowerCase();
  if (scheme === 'http:' || scheme === 'https:') return { kind: 'web', url: raw };
  if (scheme === 'mailto:') {
    // `mailto:a@b.example?subject=hi` — the address is what precedes the query,
    // and it arrives percent-encoded often enough to be worth decoding.
    const body = raw.slice('mailto:'.length).split('?')[0];
    let addr: string;
    try {
      addr = decodeURIComponent(body);
    } catch {
      addr = body;
    }
    addr = addr.trim();
    return addr ? { kind: 'mail', addr } : { kind: 'blocked' };
  }
  return { kind: 'blocked' };
}

/**
 * The destination of whatever link is under the pointer, or null.
 *
 * Shown because the link opens in a browser, so there is no address bar on the
 * way to check it against — and because link text that disagrees with its
 * destination is the whole of phishing. The reader is entitled to look first.
 */
export function useHoveredLink(): string | null {
  const [href, setHref] = useState<string | null>(null);
  useEffect(() => {
    function onMessage(e: MessageEvent) {
      const url = (e.data as { petrelHover?: unknown })?.petrelHover;
      if (typeof url !== 'string') return;
      setHref(url || null);
    }
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, []);
  return href;
}

/**
 * Listens for the link clicks a message frame forwards.
 *
 * Registered once per window rather than per message: the frames post to the
 * same parent, and one listener holding the policy beats the same decision
 * copied down through every component that happens to render a body.
 */
export function useMessageLinks(
  onMailto: (addr: string) => void,
  /** Asked before opening a link whose spelling disguises where it goes.
   *  Without a handler the link simply opens, which is the behaviour every
   *  caller had before this existed. */
  onRisky?: (risk: HomographRisk, open: () => void) => void,
) {
  useEffect(() => {
    function onMessage(e: MessageEvent) {
      const href = (e.data as { petrelOpen?: unknown })?.petrelOpen;
      if (typeof href !== 'string') return;
      const link = classifyLink(href);
      if (link.kind === 'web') {
        const risk = homographRisk(link.url);
        if (risk && onRisky) {
          onRisky(risk, () => void api.openExternal(link.url));
          return;
        }
        void api.openExternal(link.url);
      } else if (link.kind === 'mail') onMailto(link.addr);
    }
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [onMailto, onRisky]);
}
