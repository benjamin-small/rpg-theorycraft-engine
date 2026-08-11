// Weld the Vite output into one self-contained HTML file that works from a
// file:// URL. Both Rust engines normally fetch their own .wasm asset; here we
// decode those bytes before the application bundle starts and pass them to the
// generated initializers directly.
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const webRoot = join(dirname(fileURLToPath(import.meta.url)), '..');
const dist = join(webRoot, 'dist');
const assetDir = join(dist, 'assets');
const assets = readdirSync(assetDir);

const exactlyOne = (description, predicate) => {
  const matches = assets.filter(predicate);
  if (matches.length !== 1) {
    throw new Error(`expected one ${description} in ${assetDir}, found: ${matches.join(', ')}`);
  }
  return matches[0];
};

const jsName = exactlyOne('JavaScript bundle', (name) => name.endsWith('.js'));
const cssName = exactlyOne('stylesheet', (name) => name.endsWith('.css'));
const btermWasmName = exactlyOne(
  'browser-terminal Wasm module',
  (name) => name.startsWith('bterm_wasm_bg-') && name.endsWith('.wasm'),
);
const rtceWasmName = exactlyOne(
  'rtce Wasm module',
  (name) => name.startsWith('rtce_wasm_bg-') && name.endsWith('.wasm'),
);

const html = readFileSync(join(dist, 'index.html'), 'utf8');
const js = readFileSync(join(assetDir, jsName), 'utf8');
const css = readFileSync(join(assetDir, cssName), 'utf8');
const btermWasm = readFileSync(join(assetDir, btermWasmName));
const rtceWasm = readFileSync(join(assetDir, rtceWasmName));

if (/(^|\n)\s*import\s[^;]*from\s*["']/.test(js)) {
  throw new Error('bundle still contains an external import; cannot build a file:// demo');
}

const safeJs = js.replace(/<\/script/gi, '<\\/script');
const safeCss = css.replace(/<\/style/gi, '<\\/style');
let stripped = html.replace(/<script\b[^>]*\bsrc="[^"]*"[^>]*>\s*<\/script>/gi, '');
stripped = stripped.replace(/<link\b[^>]*\brel="stylesheet"[^>]*>/gi, '');

if (stripped === html) {
  throw new Error('did not find the Vite script and stylesheet tags to inline');
}

const bootstrap = `
<style>${safeCss}</style>
<script type="module">
  const decode = (base64) => Uint8Array.from(atob(base64), (character) => character.charCodeAt(0));
  globalThis.__BTERM_WASM__ = decode("${btermWasm.toString('base64')}");
  globalThis.__RTCE_WASM__ = decode("${rtceWasm.toString('base64')}");
</script>
<script type="module">
${safeJs}
</script>
</body>`;

const outDir = join(webRoot, 'dist-standalone');
const outFile = join(outDir, 'rtce-field-guide.html');
mkdirSync(outDir, { recursive: true });
writeFileSync(outFile, stripped.replace('</body>', bootstrap));

const megabytes = (bytes) => `${(bytes / 1024 / 1024).toFixed(2)} MB`;
console.log(`wrote ${outFile}`);
console.log(`  browser-terminal wasm: ${megabytes(btermWasm.length)}`);
console.log(`  rtce wasm:             ${megabytes(rtceWasm.length)}`);
console.log(`  single HTML file:      ${megabytes(readFileSync(outFile).length)}`);
