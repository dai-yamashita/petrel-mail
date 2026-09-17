/* The locale the chrome ends up in, which is also what goes on `<html lang>`.
 *
 * That attribute is not decoration in WebKit, the engine the app ships in: it
 * decides which font draws a Han character, and Japanese and Simplified Chinese
 * draw several of the same code points differently. A wrong answer here is a
 * screen of wrong glyph forms, so the mapping is worth pinning.
 */
import { describe, expect, it, afterEach, vi } from 'vitest';
import { resolveLocale } from './settings';

const platform = (languages: string[]) => {
  vi.stubGlobal('navigator', { languages, language: languages[0] ?? 'en' });
};

afterEach(() => vi.unstubAllGlobals());

describe('resolveLocale', () => {
  it('passes through a locale that ships', () => {
    for (const tag of ['en', 'ja', 'ko', 'zh-Hans', 'pt-BR', 'de', 'es', 'fr']) {
      expect(resolveLocale(tag)).toBe(tag);
    }
  });

  it('falls back to the base language when only that ships', () => {
    // de-AT is not a locale we carry; German is.
    expect(resolveLocale('de-AT')).toBe('de');
  });

  it('gives English rather than a screen of ids for a language we lack', () => {
    expect(resolveLocale('is')).toBe('en');
  });

  it('follows the platform when asked to', () => {
    platform(['ja-JP', 'en-GB']);
    expect(resolveLocale('system')).toBe('ja');
    platform(['ko-KR']);
    expect(resolveLocale('system')).toBe('ko');
    platform(['is-IS']);
    expect(resolveLocale('system')).toBe('en');
  });

  it('reaches the Simplified Chinese that ships', () => {
    // A platform says `zh-CN`, and the locale is `zh-Hans`. There is no `zh` in
    // between, so matching only the tag or its base landed every Chinese Mac on
    // English — a translation that shipped and could never be selected.
    for (const tag of ['zh-Hans-CN', 'zh-CN', 'zh-SG', 'zh-Hans']) {
      platform([tag]);
      expect(resolveLocale('system')).toBe('zh-Hans');
    }
  });

  it('gives Traditional Chinese English rather than Simplified characters', () => {
    // We ship no zh-Hant. Handing a Taiwanese reader Simplified text would be
    // worse than answering honestly in English.
    for (const tag of ['zh-TW', 'zh-Hant-TW', 'zh-HK']) {
      platform([tag]);
      expect(resolveLocale('system')).toBe('en');
    }
  });

  it('answers the same as before for every tag that already worked', () => {
    const unchanged: Record<string, string> = {
      'ja-JP': 'ja', 'ko-KR': 'ko', 'de-AT': 'de', 'pt-BR': 'pt-BR',
      'fr-CA': 'fr', 'es-MX': 'es', 'en-GB': 'en', 'is-IS': 'en',
    };
    for (const [tag, want] of Object.entries(unchanged)) {
      platform([tag]);
      expect(resolveLocale('system')).toBe(want);
    }
  });

  it('prefers an earlier platform language over a later one', () => {
    platform(['is-IS', 'ja-JP', 'de-DE']);
    expect(resolveLocale('system')).toBe('ja');
  });

  it('never answers with a tag a browser would reject', () => {
    // Whatever comes back is written to `<html lang>`, so it has to be a
    // language tag and not a UI token like "system".
    for (const setting of ['system', '', 'ja', 'nonsense']) {
      expect(resolveLocale(setting)).not.toBe('system');
      expect(resolveLocale(setting)).toMatch(/^[a-z]{2}(-[A-Za-z]+)?$/);
    }
  });
});
