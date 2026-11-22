// Every fixed English label in index.html must have a Turkish counterpart, so
// the Turkish interface never shows English by accident. A visible text node
// passes when one of these holds:
//   - its element has data-en and data-tr (applyLanguage swaps them);
//   - its element's text is replaced at runtime by main.js or another UI script
//     (matched by id), so the runtime translation applies;
//   - the text is a key of main.js's Turkish dictionary (trText);
//   - it is a name that is not translated: a brand, a theme, a key name, a
//     language name or a placeholder.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const ui = path.resolve(__dirname, '../crates/desktop/ui');
const html = fs.readFileSync(path.join(ui, 'index.html'), 'utf8');
const scripts = fs.readdirSync(ui).filter(name => name.endsWith('.js'))
  .map(name => fs.readFileSync(path.join(ui, name), 'utf8')).join('\n');

// Ids whose text a script assigns at runtime.
const runtimeIds = new Set();
for (const match of scripts.matchAll(/\$\(\s*["']#([\w-]+)["']\s*\)\s*\.\s*(textContent|innerText)\s*=/g)) runtimeIds.add(match[1]);
// Elements bound to a variable first (const hint = $("#id")), then assigned through it.
for (const bind of scripts.matchAll(/(?:const|let)\s+(\w+)\s*=\s*\$\(\s*["']#([\w-]+)["']\s*\)/g)) {
  if (new RegExp("\\b" + bind[1] + "\\.(textContent|innerText)\\s*=").test(scripts)) runtimeIds.add(bind[2]);
}
for (const match of scripts.matchAll(/getElementById\(\s*["']([\w-]+)["']\s*\)\.(textContent|innerText)\s*=/g)) runtimeIds.add(match[1]);

// Turkish dictionary keys in main.js (the trText object and its extensions).
const dictionary = new Set();
for (const match of scripts.matchAll(/"((?:[^"\\]|\\.)+)"\s*:\s*"(?:[^"\\]|\\.)*"/g)) dictionary.add(match[1].replace(/\\"/g, '"'));

// Names that stay the same in both languages.
const untranslated = new Set([
  'English', 'Türkçe', 'GhostChat', 'GhostShare', 'Watch Party', 'Sakura', 'Aurora',
  'NVIDIA Broadcast AI', 'Ctrl', 'Shift', 'Alt', 'Esc', 'Enter', 'Space', 'K', 'G', '@ghost', '@username',
]);

const body = html.slice(html.indexOf('<body'));
const visible = body
  .replace(/<script[\s\S]*?<\/script>/g, '')
  .replace(/<style[\s\S]*?<\/style>/g, '');

const problems = [];
// Match an element start tag with its direct text content (no nested tags).
const element = /<([a-z][\w-]*)([^>]*)>([^<]*)</gi;
for (const match of visible.matchAll(element)) {
  const [, tag, attributes, rawText] = match;
  const text = rawText.trim();
  if (!text || !/[A-Za-z]/.test(text)) continue;
  if (/\bdata-en=|\bdata-tr=/.test(attributes)) continue;
  const id = (attributes.match(/\bid="([^"]+)"/) || [])[1];
  if (id && runtimeIds.has(id)) continue;
  if (dictionary.has(text) || untranslated.has(text)) continue;
  if (/^[\d\s.,:;%$€()+\-–…]+$/.test(text)) continue;
  problems.push(`<${tag}${id ? ` id="${id}"` : ''}>${text}`);
}

assert.deepEqual(problems, [], `Visible English without a Turkish counterpart:\n${problems.join('\n')}`);
console.log('ui translation tests passed');
