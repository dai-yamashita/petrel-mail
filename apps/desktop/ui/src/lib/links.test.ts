import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  classifyLink,
  homographRisk,
  linkReadout,
  linkRisk,
  unicodeHost,
  useMessageLinks,
  type HomographRisk,
} from './links';

// The click path below runs `useMessageLinks` outside React. Its one effect is
// run at once, which is all it does: put a listener on the window. And the
// browser it would hand a link to is a list.
const opened = vi.hoisted(() => [] as string[]);
vi.mock('./api', () => ({
  api: {
    openExternal: async (url: string) => {
      opened.push(url);
    },
  },
}));
vi.mock('react', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react')>()),
  useEffect: (effect: () => void) => {
    effect();
  },
}));

describe('classifyLink', () => {
  it('sends web links to the browser', () => {
    expect(classifyLink('https://example.com/a?b=1')).toEqual({
      kind: 'web',
      url: 'https://example.com/a?b=1',
    });
    expect(classifyLink('http://old.example/x.html').kind).toBe('web');
  });

  it('keeps mail links in Petrel, with just the address', () => {
    expect(classifyLink('mailto:sam@example.com')).toEqual({ kind: 'mail', addr: 'sam@example.com' });
    expect(classifyLink('mailto:sam@example.com?subject=Hi%20there')).toEqual({
      kind: 'mail',
      addr: 'sam@example.com',
    });
    expect(classifyLink('mailto:a%2Bb@example.com')).toEqual({ kind: 'mail', addr: 'a+b@example.com' });
  });

  it('opens nothing else, whoever sent it', () => {
    for (const href of [
      'javascript:alert(1)',
      'file:///etc/passwd',
      'data:text/html,<script>x</script>',
      'petrel-msg://localhost/message/1',
      'vscode://file/Users/me/.ssh/id_rsa',
      'mailto:',
      '',
      'about:blank',
    ]) {
      expect(classifyLink(href), href).toEqual({ kind: 'blocked' });
    }
  });

  it('is not fooled by case or padding', () => {
    expect(classifyLink('  HTTPS://Example.com/  ').kind).toBe('web');
    expect(classifyLink('JavaScript:alert(1)').kind).toBe('blocked');
  });
});

describe('homographRisk', () => {
  it('says nothing about ordinary links', () => {
    expect(homographRisk('https://apple.com/store')).toBeNull();
    expect(homographRisk('http://localhost:5199/x')).toBeNull();
    expect(homographRisk('not a url')).toBeNull();
  });

  it('catches a Latin name spelled with another alphabet', () => {
    // "аpple.com" — the first letter is Cyrillic а, and the rest is Latin.
    const risk = homographRisk('https://аpple.com/login');
    expect(risk).not.toBeNull();
    expect(risk!.reason).toBe('mixed-script');
    expect(risk!.asPunycode).toContain('xn--');
    expect(risk!.asTyped).toBe('аpple.com');
  });

  it('catches a name built from lookalikes even without an ASCII tld', () => {
    // With a Latin .com the mixing itself gives it away, so the interesting
    // case is a name where every part is Cyrillic — nothing to mix with,
    // and every letter still chosen to pass for a Latin one.
    const risk = homographRisk('https://раураӏ.рф');
    expect(risk?.reason).toBe('latin-lookalike');
    // And the everyday version. The .com is a label of its own, so the name
    // is not mixing scripts; it is a Cyrillic word that reads as a Latin one.
    expect(homographRisk('https://раураӏ.com')?.reason).toBe('latin-lookalike');
    expect(homographRisk('https://сосо.com')?.reason).toBe('latin-lookalike');
  });

  it('catches a Latin stand-in letter in an ordinary name', () => {
    expect(homographRisk('https://ɡoogle.com')?.reason).toBe('latin-lookalike');
  });

  it('leaves honest international domains alone', () => {
    // A Japanese domain is somebody's real address, not a disguise.
    expect(homographRisk('https://日本語.jp')).toBeNull();
    // As is a German one with an umlaut.
    expect(homographRisk('https://münchen.de')).toBeNull();
  });

  it('leaves a Cyrillic or Greek name alone, whatever its tld', () => {
    // Each has letters that look Latin, but not only those, so none reads as
    // a Latin word. Questioning them would question the language itself.
    expect(homographRisk('https://яндекс.рф')).toBeNull();
    expect(homographRisk('https://пример.com')).toBeNull();
    expect(homographRisk('https://почта.рус')).toBeNull();
    expect(homographRisk('https://ενα.gr')).toBeNull();
  });
});

describe('homographRisk, given what the message frame really sends', () => {
  // The frame posts the anchor's `href` property, which the browser has
  // already turned into the ASCII form DNS uses. Every test above hands the
  // check a Unicode spelling the running code never receives; these hand it
  // the one it does.
  it('catches a Latin name spelled with another alphabet', () => {
    const risk = homographRisk('https://xn--pple-id-1fg.example/login');
    expect(risk?.reason).toBe('mixed-script');
    expect(risk?.asTyped).toBe('\u0430pple-id.example');
    expect(risk?.asPunycode).toBe('xn--pple-id-1fg.example');
  });

  it('catches a name built from lookalikes', () => {
    expect(homographRisk('https://xn--80aa0cbo65f.xn--p1ai/')?.reason).toBe('latin-lookalike');
    expect(homographRisk('https://xn--80aa0cbo65f.com/')?.reason).toBe('latin-lookalike');
    expect(homographRisk('https://xn--pple-43d.com/')?.reason).toBe('mixed-script');
  });

  it('still leaves honest international domains alone', () => {
    expect(homographRisk('https://xn--wgv71a119e.jp/')).toBeNull();
    expect(homographRisk('https://xn--mnchen-3ya.de/')).toBeNull();
    expect(homographRisk('https://xn--d1acpjx3f.xn--p1ai/')).toBeNull();
    expect(homographRisk('https://xn--e1afmkfd.com/')).toBeNull();
  });
});

describe('linkRisk', () => {
  // Everything before the @ is a user name, and the link goes to what follows
  // it. Spelled as another site, it is the disguise, so it is asked about.
  it('asks about a user name posing as the site', () => {
    expect(
      linkRisk(
        'https://www.paypal.com.account-security.session-checkpoint.verification-center@evil.example/signin',
      ),
    ).toEqual({
      asTyped: 'www.paypal.com.account-security.session-checkpoint.verification-center',
      asPunycode: 'evil.example',
      reason: 'userinfo',
    });
    expect(linkRisk('https://www.paypal.com@evil.example/')?.asPunycode).toBe('evil.example');
  });

  it('asks about a password too, and quotes the name as the link spells it', () => {
    expect(linkRisk('https://paypal.com:secure@evil.example/')?.asTyped).toBe('paypal.com:secure');
    // Percent-encoded as the frame hands it over, so nothing in it can
    // reorder the sentence it is quoted in.
    expect(linkRisk('https://accounts.google.com%2Fsignin@evil.example/')?.asTyped).toBe(
      'accounts.google.com%2Fsignin',
    );
  });

  it('names the host in the form DNS uses, whatever it is spelled in', () => {
    expect(linkRisk('https://paypal.com@xn--pple-43d.com/')).toEqual({
      asTyped: 'paypal.com',
      asPunycode: 'xn--pple-43d.com',
      reason: 'userinfo',
    });
  });

  it('still asks about a look-alike host, and nothing else', () => {
    expect(linkRisk('https://xn--pple-id-1fg.example/login')?.reason).toBe('mixed-script');
    expect(linkRisk('https://example.com/a?b=1')).toBeNull();
    // An @ after the host is part of the path, not a user name.
    expect(linkRisk('https://example.com/people/@dana')).toBeNull();
    expect(linkRisk('not a url')).toBeNull();
  });
});

describe('linkReadout', () => {
  it('leaves the user name out, so the host comes first', () => {
    expect(
      linkReadout(
        'https://www.paypal.com.account-security.session-checkpoint.verification-center@evil.example/signin',
      ),
    ).toBe('https://evil.example/signin');
    expect(linkReadout('https://user:pw@evil.example:8443/a?b=1#c')).toBe('https://evil.example:8443/a?b=1#c');
  });

  it('shows every other link exactly as it is', () => {
    for (const href of [
      'https://example.com/a/very/long/path',
      'https://xn--pple-id-1fg.example/login',
      'https://example.com/people/@dana',
      'mailto:sam@example.com',
      'not a url',
    ]) {
      expect(linkReadout(href), href).toBe(href);
    }
  });
});

describe('a click on a link in a message', () => {
  // The path the main window and the pop-out share: the frame posts the
  // anchor's href, and `useMessageLinks` opens it or asks first.
  let asked: HomographRisk[];
  let answer: (() => void) | null;
  beforeEach(() => {
    vi.stubGlobal('window', new EventTarget());
    opened.length = 0;
    asked = [];
    answer = null;
    useMessageLinks(
      () => {},
      (risk, open) => {
        asked.push(risk);
        answer = open;
      },
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  const click = (href: string) => {
    window.dispatchEvent(new MessageEvent('message', { data: { petrelOpen: href } }));
  };

  it('asks before opening a link whose user name poses as the site', () => {
    const href =
      'https://www.paypal.com.account-security.session-checkpoint.verification-center@evil.example/signin';
    click(href);
    expect(opened).toEqual([]);
    expect(asked.map((r) => [r.reason, r.asPunycode])).toEqual([['userinfo', 'evil.example']]);
    // "Open anyway" opens exactly what was clicked.
    answer?.();
    expect(opened).toEqual([href]);
  });

  it('asks about the short form as well', () => {
    click('https://www.paypal.com@evil.example/signin');
    expect(opened).toEqual([]);
    expect(asked).toHaveLength(1);
  });

  it('still opens an ordinary link at once', () => {
    click('https://example.com/a/very/long/path');
    expect(asked).toEqual([]);
    expect(opened).toEqual(['https://example.com/a/very/long/path']);
  });

  it('still asks about a look-alike host', () => {
    click('https://xn--pple-id-1fg.example/login');
    expect(opened).toEqual([]);
    expect(asked.map((r) => r.reason)).toEqual(['mixed-script']);
  });
});

describe('unicodeHost', () => {
  // Pairs read from Node's url.domainToUnicode, which implements the same
  // RFC 3492 decoding browsers use.
  const PAIRS: [string, string][] = [
    ['xn--pple-id-1fg.example', '\u0430pple-id.example'],
    ['xn--mnchen-3ya.de', 'm\u00fcnchen.de'],
    ['xn--bcher-kva.example', 'b\u00fccher.example'],
    ['xn--d1acpjx3f.xn--p1ai', '\u044f\u043d\u0434\u0435\u043a\u0441.\u0440\u0444'],
    ['xn--wgv71a119e.jp', '\u65e5\u672c\u8a9e.jp'],
    ['xn--hxargifdar.gr', '\u03b5\u03bb\u03bb\u03b7\u03bd\u03b9\u03ba\u03ac.gr'],
    ['xn--strae-oqa.de', 'stra\u00dfe.de'],
    ['xn--maana-pta.com', 'ma\u00f1ana.com'],
    ['xn--r8jz45g.xn--zckzah', '\u4f8b\u3048.\u30c6\u30b9\u30c8'],
    ['ab--c.example', 'ab--c.example'],
    ['apple.com', 'apple.com'],
  ];
  it('decodes each label as the platform does', () => {
    for (const [ascii, unicode] of PAIRS) expect(unicodeHost(ascii), ascii).toBe(unicode);
  });

  it('leaves a label it cannot decode as it was', () => {
    expect(unicodeHost('xn--.example')).toBe('xn--.example');
    expect(unicodeHost('xn--a-\u0100.example')).toBe('xn--a-\u0100.example');
  });
});
