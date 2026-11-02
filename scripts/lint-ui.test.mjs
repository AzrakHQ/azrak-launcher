import assert from 'node:assert/strict';
import { test } from 'node:test';
import { length, lint } from './lint-ui.mjs';

test('lengths in either scale', () => {
  assert.deepEqual(length('(104px * S.k)'), { px: 104, scale: 'k' });
  assert.deepEqual(length('(19px * S.t)'), { px: 19, scale: 't' });
  assert.equal(length('parent.height / 2'), null);
});

test('a number taller than its row is caught (the cut-off 0 on the Downloads page)', () => {
  const ui = `HorizontalLayout {
    height: (96px * S.k);
    Text { text: "0"; font-size: (104px * S.k); }
}`;
  assert.equal(lint(ui).length, 1);
  assert.equal(lint(ui.replace('96px', '126px')).length, 0);
});

test('text scale against a layout-scaled box is checked at the smallest window', () => {
  // 32px (a 38px line) fits 40px at 1920×1080, but S.t outgrows S.k on 1280×720.
  const ui = 'Rectangle { height: (40px * S.k); Text { font-size: (32px * S.t); } }';
  assert.equal(lint(ui).length, 1);
  assert.equal(lint(ui.replace('S.t', 'S.k')).length, 0);
});

test('a column shares its height out, wrapped text grows, and allow skips a line', () => {
  assert.equal(lint('Rectangle { height: (40px * S.k); VerticalLayout { Text { font-size: (36px * S.k); } Text { font-size: (36px * S.k); } } }').length, 0);
  assert.equal(lint('Rectangle { height: (40px * S.k); Text { font-size: (36px * S.k); wrap: word-wrap; } }').length, 0);
  assert.equal(lint('Rectangle { height: (40px * S.k); Text { font-size: (36px * S.k); } // ui-lint: allow (on purpose)\n}').length, 0);
});
