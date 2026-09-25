import { getFontFace } from '$lib/backend';
import { info, warn } from '$lib/utils/log';

/**
 * Installs the user's chosen fonts into the document.
 *
 * A font added by the user lives on disk, not on a server, so the webview
 * cannot reach it by path. The bytes come over IPC once per font and are turned
 * into a blob URL: a `data:` URL of several megabytes would work, but it would
 * sit in a stylesheet rule and be re-parsed on every change, which is the shape
 * of problem the backdrop already taught us to avoid.
 *
 * The faces go in through the `FontFace` API rather than an `@font-face` rule,
 * because a rule that cannot fetch its font fails silently: the text simply
 * keeps the fallback, and nothing anywhere says why. `load()` rejects instead —
 * on a blocked fetch, a content-security policy that forgot `font-src`, or a
 * file that is not really a font — so the reason reaches the log.
 *
 * Two faces are installed under fixed family names and the CSS variables
 * `--font-ui` and `--font-digits` point at them. Nothing else in the app needs
 * to know which file is in use.
 */

const UI_FAMILY = 'PomotroidUI';
const DIGITS_FAMILY = 'PomotroidDigits';

type Slot = 'ui' | 'digits';

/** What is installed per slot, kept so it can be taken back out again. */
type Installed = { name: string; url: string; face: FontFace };
const installed: Record<Slot, Installed | null> = { ui: null, digits: null };

function familyOf(slot: Slot): string {
  return slot === 'ui' ? UI_FAMILY : DIGITS_FAMILY;
}

function variableOf(slot: Slot): string {
  return slot === 'ui' ? '--font-ui' : '--font-digits';
}

/** The stack each slot falls back to when no font is chosen. */
function fallbackOf(slot: Slot): string {
  return slot === 'ui'
    ? "'Mona Sans', system-ui, sans-serif"
    : "'Mona Sans Mono', ui-monospace, monospace";
}

/** Point the CSS variables at whatever is installed. */
function render() {
  const root = document.documentElement;
  for (const slot of ['ui', 'digits'] as Slot[]) {
    const stack = installed[slot] ? `'${familyOf(slot)}', ${fallbackOf(slot)}` : fallbackOf(slot);
    root.style.setProperty(variableOf(slot), stack);
  }
}

function release(slot: Slot) {
  const current = installed[slot];
  if (!current) return;
  document.fonts.delete(current.face);
  URL.revokeObjectURL(current.url);
  installed[slot] = null;
}

function toObjectUrl(dataUrl: string, mime: string): string {
  const comma = dataUrl.indexOf(',');
  const binary = atob(dataUrl.slice(comma + 1));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return URL.createObjectURL(new Blob([bytes], { type: mime }));
}

/**
 * Make `name` the font for `slot`; an empty name restores the bundled face.
 *
 * Re-installing the font already in the slot is skipped, so a `settings:changed`
 * broadcast about something else does not re-fetch megabytes of font.
 */
export async function useFont(slot: Slot, name: string): Promise<void> {
  const wanted = name.trim();
  if (installed[slot]?.name === wanted) return;
  if (!wanted) {
    release(slot);
    render();
    return;
  }

  let url: string | null = null;
  try {
    const source = await getFontFace(wanted);
    if (!source) {
      // Chosen but gone. The slot falls back rather than clearing the setting
      // from here, which would fight the backend over the same value.
      await warn(`[fonts] '${wanted}' is not installed; using the bundled face`);
      release(slot);
      render();
      return;
    }

    url = toObjectUrl(source.data_url, `font/${source.format}`);
    const face = new FontFace(familyOf(slot), `url("${url}")`);
    // Rejects on a blocked fetch or a file the engine cannot parse, which is
    // the whole reason for not using an `@font-face` rule here.
    await face.load();

    release(slot);
    document.fonts.add(face);
    installed[slot] = { name: wanted, url, face };
    url = null;
    render();
    await info(`[fonts] ${slot} is now ${wanted}`);
  } catch (err) {
    if (url) URL.revokeObjectURL(url);
    await warn(`[fonts] could not use '${wanted}' for ${slot}: ${err}`);
    release(slot);
    render();
  }
}

/** Apply both slots from a settings snapshot. */
export async function applyFonts(fontUi: string, fontDigits: string): Promise<void> {
  await Promise.all([useFont('ui', fontUi), useFont('digits', fontDigits)]);
}
