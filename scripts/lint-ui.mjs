#!/usr/bin/env node
// Static checks of the .slint files for text that can't fit its box: a font whose line is taller
// than the fixed height around it gets cut (a 104px number in a 96px row). Sizes in S.t (text
// scale) grow faster than boxes in S.k (layout scale) on small windows, so those are checked at
// the worst ratio. Fast and deterministic: run it before every release.
//
// Usage: node scripts/lint-ui.mjs [files…]   (default: ui/*.slint). Exit 1 on problems.
// A line ending in `// ui-lint: allow (reason)` is skipped.
import { readFileSync, readdirSync, realpathSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
// A line of text is about 1.2 × its font size (Inter 1.21, DejaVu Sans 1.16).
const LINE = 1.2;
// S.t is at least 0.75 (present.rs TEXT_SCALE_MIN) while S.k follows the window: on the
// smallest window the launcher allows (640×360, app.slint min-height; k = 0.333) text is 2.25×
// bigger relative to boxes.
const T_OVER_K = 0.75 / (360 / 1080);

/// Elements with their properties and children, from Slint source. Not a full parser: enough
/// for `Type { prop: value; … }`, `id := Type {`, `if cond: Type {`, `for x in list: Type {`.
export function parse(text) {
  const top = { type: 'root', props: {}, children: [], line: 1 };
  const stack = [top];
  let i = 0, line = 1, start = 0, startLine = 1;
  const statement = () => text.slice(start, i).trim();
  while (i < text.length) {
    const c = text[i];
    if (c === '\n') { line++; i++; continue; }
    // Comments: skipped, and a statement that hasn't started yet starts after them.
    if (c === '/' && (text[i + 1] === '/' || text[i + 1] === '*')) {
      const fresh = text.slice(start, i).trim() === '';
      if (text[i + 1] === '/') { while (i < text.length && text[i] !== '\n') i++; }
      else {
        const end = text.indexOf('*/', i + 2);
        const stop = end < 0 ? text.length : end + 2;
        line += (text.slice(i, stop).match(/\n/g) || []).length;
        i = stop;
      }
      if (fresh) start = i;
      continue;
    }
    if (c === '"') { i++; while (i < text.length && text[i] !== '"') { if (text[i] === '\\') i++; i++; } i++; continue; }
    if (c === '{') {
      const head = statement();
      // The element's type: the last name before the brace (after `:=`, `if …:`, `for …:`).
      const m = head.match(/([A-Za-z_][\w-]*)\s*$/);
      // `animate x, y { … }` is an animation, not an element named after its last property.
      const type = /^animate\b/.test(head) ? 'animate' : m ? m[1] : '?';
      const el = { type, head, props: {}, children: [], line: startLine, parent: stack.at(-1) };
      stack.at(-1).children.push(el);
      stack.push(el);
      i++; start = i; startLine = line;
      continue;
    }
    if (c === '}') {
      if (stack.length > 1) stack.pop();
      i++; start = i; startLine = line;
      continue;
    }
    if (c === ';') {
      const s = statement();
      const m = s.match(/^([a-z][\w-]*)\s*:\s*([\s\S]+)$/);
      if (m && !s.includes('=>')) stack.at(-1).props[m[1]] = { value: m[2].trim(), line: startLine };
      i++; start = i; startLine = line;
      continue;
    }
    // A statement starts on the line of its first character.
    if (!/\s/.test(c) && text.slice(start, i).trim() === '') startLine = line;
    i++;
  }
  return top;
}

/// A length like `(104px * S.k)`, `(19px * S.t)` or `12px`: its pixels and its scale.
export function length(value) {
  if (!value) return null;
  const v = value.replace(/^\(|\)$/g, '').trim();
  let m = v.match(/^([\d.]+)px\s*\*\s*S\.(k|t)$/);
  if (m) return { px: +m[1], scale: m[2] };
  m = v.match(/^([\d.]+)px$/);
  if (m) return { px: +m[1], scale: 'k' };
  return null;
}

/// Problems in one file: [{ line, message }].
export function lint(text) {
  const lines = text.split('\n');
  const allowed = n => /ui-lint:\s*allow/.test(lines[n - 1] || '');
  const problems = [];
  const visit = el => {
    if (el.type === 'Text' && el.props['font-size']) {
      const font = length(el.props['font-size'].value);
      const wraps = /word-wrap/.test(el.props.wrap?.value || '');
      if (font) {
        // The box it gets: its own fixed height, else the nearest fixed one around it, through
        // containers that give it their whole height (not a column that shares it out).
        // Padding (top and bottom) of a container: less room inside it.
        const vpad = e => {
          const px = name => length(e.props[name]?.value)?.px ?? 0;
          return (e.props['padding-top'] ? px('padding-top') : px('padding')) + (e.props['padding-bottom'] ? px('padding-bottom') : px('padding'));
        };
        let box = null, between = 0;
        if (el.props.height) box = { len: length(el.props.height.value), el };
        for (let p = el.parent; !box && p && p.type !== 'root'; p = p.parent) {
          if (p.props.height) { box = { len: length(p.props.height.value), el: p }; break; }
          // `animate` blocks aren't elements.
          const stacks = p.type === 'VerticalLayout' && p.children.filter(c => c.type !== 'animate').length > 1;
          if (stacks) break;
          between += vpad(p);
        }
        if (box?.len && !(wraps && box.el !== el)) {
          const ratio = font.scale === 't' && box.len.scale === 'k' ? T_OVER_K : 1;
          const need = font.px * LINE * ratio;
          // The box's own padding, and any between it and the text, leave less room.
          const room = box.el === el ? box.len.px : box.len.px - vpad(box.el) - between;
          if (need > room + 0.5 && !allowed(el.line) && !allowed(el.props['font-size'].line)) {
            problems.push({
              line: el.props['font-size'].line,
              message: `text of ${font.px}px (S.${font.scale}) needs about ${need.toFixed(0)}px of height`
                + `${ratio > 1 ? ' on the smallest windows' : ''}, but its box (line ${box.el.props.height.line}) has ${room}px (S.${box.len.scale})`,
            });
          }
        }
      }
    }
    // A fixed-height column holds all its lines (spacing aside).
    if (el.type === 'VerticalLayout' && el.props.height) {
      const box = length(el.props.height.value);
      const texts = el.children.filter(c => c.type === 'Text' && c.props['font-size']).map(c => length(c.props['font-size'].value)).filter(Boolean);
      if (box && texts.length > 1) {
        const need = texts.reduce((sum, f) => sum + f.px * LINE * (f.scale === 't' && box.scale === 'k' ? T_OVER_K : 1), 0);
        if (need > box.px + 0.5 && !allowed(el.props.height.line)) {
          problems.push({ line: el.props.height.line, message: `a column of ${texts.length} lines needs about ${need.toFixed(0)}px, but it is ${box.px}px (S.${box.scale})` });
        }
      }
    }
    el.children.forEach(visit);
  };
  visit(parse(text));
  return problems;
}

if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  const files = process.argv.slice(2).length ? process.argv.slice(2)
    : readdirSync(join(root, 'ui')).filter(f => f.endsWith('.slint')).map(f => join(root, 'ui', f));
  let count = 0;
  for (const file of files) {
    for (const p of lint(readFileSync(file, 'utf8'))) {
      console.log(`${file.replace(root + '/', '')}:${p.line}: ${p.message}`);
      count++;
    }
  }
  console.log(count ? `${count} problem${count === 1 ? '' : 's'}` : 'ui lint: no problems');
  process.exit(count ? 1 : 0);
}
