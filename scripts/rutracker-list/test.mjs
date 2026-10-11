import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chromium } from 'playwright-core';
import { readListing, mergeTopics } from './listing.mjs';
import { collectMagnetBatch, applyMagnets, normalizeGameInfo } from './magnets.mjs';

test('PS4 forum: reads forum 973 and keeps only [PS4] topics', async () => {
  const { PLATFORMS } = await import('./platforms.mjs');
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  try {
    const page = await browser.newPage();
    await page.route('https://rutracker.net/**', route => route.fulfill({ contentType: 'text/html', body: '<title>PS4</title>' }));
    await page.goto('https://rutracker.net/forum/viewforum.php?f=973');
    await page.setContent(`<title>PS4</title><table>
      <tr><td><a class="torTopic" href="viewtopic.php?t=300">[PS4] Example Game [CUSA12345]</a></td></tr>
      <tr><td><a class="torTopic" href="viewtopic.php?t=301">[PS5] Wrong console</a></td></tr>
    </table><a href="viewforum.php?f=973&amp;start=50">2</a>`);
    const result = await page.evaluate(readListing, PLATFORMS.ps4);
    assert.equal(result.status, 'ok');
    assert.deepEqual(result.topics.map(t => t.id), ['300']);
    assert.equal(result.pageUrls[1], 'https://rutracker.net/forum/viewforum.php?f=973&start=50');
    // The PS5 reader refuses the PS4 forum page.
    assert.equal((await page.evaluate(readListing, PLATFORMS.ps5)).status, 'unexpected_page');
  } finally {
    await browser.close();
  }
});

test('forum DOM reader: pagination, metadata, duplicates and challenge handling', async () => {
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  try {
    const page = await browser.newPage();
    await page.route('https://rutracker.net/**', route => route.fulfill({
      contentType: 'text/html', body: '<html><head><title>PS5</title></head><body></body></html>',
    }));
    await page.goto('https://rutracker.net/forum/viewforum.php?f=546');
    await page.setContent(`<title>PS5</title><table>
      <tr><td><a class="torTopic" href="viewtopic.php?t=100">[PS5] Example &amp; Game</a>
        <a class="topicAuthor">Author</a></td><td class="vf-col-tor">
        <span class="seedmed"><b>1,234</b></span><span class="leechmed"><b>5</b></span>
        <a class="dl-stub" href="dl.php?t=100">2.1&nbsp;GB</a></td></tr>
      <tr><td><a class="torTopic" href="viewtopic.php?t=100">[PS5] Example &amp; Game</a></td></tr>
      <tr><td><a class="torTopic" href="viewtopic.php?t=99">Forum rules</a></td></tr>
      <tr><td><a class="torTopic" href="https://evil.test/forum/viewtopic.php?t=200">[PS5] Bad origin</a></td></tr>
    </table><a href="viewforum.php?f=546&amp;start=50">2</a>
    <a href="viewforum.php?f=546&amp;start=100">3</a><a href="viewforum.php?f=546&amp;start=600">13</a>
    <a href="viewforum.php?f=999&amp;start=10000">Unrelated</a>`);
    let result = await page.evaluate(readListing);
    assert.equal(result.status, 'ok');
    assert.equal(result.topics.length, 1);
    assert.equal(result.topics[0].title, '[PS5] Example & Game');
    assert.equal(result.topics[0].url, 'https://rutracker.net/forum/viewtopic.php?t=100');
    assert.equal(result.topics[0].seeders, 1234);
    assert.equal(result.topics[0].leechers, 5);
    assert.equal(result.topics[0].size, '2.1 GB');
    assert.equal(result.topics[0].author, 'Author');
    assert.equal(result.pageUrls.length, 13);
    assert.equal(result.pageUrls[12], 'https://rutracker.net/forum/viewforum.php?f=546&start=600');
    // A repeated sticky topic must not inflate the count across pages.
    assert.equal(mergeTopics([result, result]).length, 1);
    await page.setContent('<title>Just a moment...</title><form id="challenge-form"></form>');
    result = await page.evaluate(readListing);
    assert.equal(result.status, 'verification_required');
    await page.setContent('<form id="login-form-full"></form>');
    assert.equal((await page.evaluate(readListing)).status, 'login_required');
    await page.setContent('<title>Unknown</title>');
    assert.equal((await page.evaluate(readListing)).status, 'unrecognized_listing');
    await page.goto('https://rutracker.net/forum/viewforum.php?f=999');
    assert.equal((await page.evaluate(readListing)).status, 'unexpected_page');
  } finally { await browser.close(); }
});

test('observed release label variants are normalized without copying prose', () => {
  const info = normalizeGameInfo({ name: 'Example Game', fields: {
    'Технические данныеДата выпуска': '30.09.2026', 'Формат игры': 'folder',
    'Язык интерфейса (Субтитры)': 'Английский', 'Минимальная версий прошивки': '4.00',
    'Возрастное ограничение': '16+', 'Мультиплеер (локальный)': 'Нет',
    'Promotional sentence mistaken for a label': 'Must not be copied',
    'Call of Duty': 'This is a title, not a metadata label',
  } });
  assert.equal(info.release_date, '30.09.2026');
  assert.equal(info.release_year, '2026');
  assert.equal(info.format, 'folder');
  assert.equal(info.minimum_firmware, '4.00');
  assert.equal(info.interface_languages, 'Английский');
  assert.equal(info.local_multiplayer, 'Нет');
  assert.equal(info.fields['Promotional sentence mistaken for a label'], undefined);
  assert.equal(info.fields['Call of Duty'], undefined);
});

test('magnet enrichment: official links, missing links, stopping and metadata preservation', async () => {
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  const magnet = `magnet:?xt=urn:btih:${'a'.repeat(40)}&dn=Example%20Game`;
  const requests = [];
  try {
    const page = await browser.newPage();
    await page.route('https://rutracker.net/**', route => {
      const id = new URL(route.request().url()).searchParams.get('t');
      if (id) requests.push(id);
      const body = id === '102' ? '<title>Just a moment...</title><form id="challenge-form"></form>'
        : id === '104' ? `<h1 id="topic-title">Bad link</h1><a class="magnet-link" data-topic_id="104" href="magnet:?xt=urn:btih:invalid">Bad</a>`
        : `<h1 id="topic-title">Example</h1><div class="post_body"><b>Example Game</b><br>
          <b>Год выпуска</b>: 2023<br><b>Жанр</b>: Action<br><b>Код диска</b>: PPSA12345<br>
          <b>Версия игры</b>: 1.05<br><b>Минимальная версия прошивки</b>: <b>4.00</b><br>
          <b>Язык интерфейса</b>: Русский, Английский<br><b>Описание</b>: Promotional paragraph, not copied.
          <div class="sp-wrap"><b>Reply author</b>: Ignore this</div>
          <img class="postImg" src="https://example.com/cover.png"><var class="postImg" title="https://example.com/screenshot.jpg"></var>
          </div><div class="post_body"><b>Код диска</b>: PPSA99999</div><a href="${magnet}">Reply link (ignore)</a>`
          + (id === '100' ? `<a class="magnet-link" data-topic_id="999" href="${magnet}">Wrong topic</a><a class="magnet-link" data-topic_id="100" href="${magnet.replaceAll('&', '&amp;')}">Magnet</a>` : '');
      return route.fulfill({ contentType: 'text/html; charset=utf-8', body });
    });
    await page.goto('https://rutracker.net/forum/viewforum.php?f=546');
    const batch = await page.evaluate(collectMagnetBatch, { topicIds: ['100', '101', '102', '103'], delayMs: 0 });
    assert.equal(batch.complete, false);
    assert.deepEqual(requests, ['100', '101', '102']);
    assert.equal(batch.results[0].magnet, magnet);
    assert.equal(batch.results[0].magnet_status, 'available');
    assert.equal(batch.results[0].game_info_status, 'available');
    assert.equal(batch.results[0].game_info.release_year, '2023');
    assert.equal(batch.results[0].game_info.title_id, 'PPSA12345');
    assert.equal(batch.results[0].game_info.minimum_firmware, '4.00');
    assert.equal(batch.results[0].game_info.interface_languages, 'Русский, Английский');
    assert.equal(batch.results[0].game_info.fields['Описание'], undefined);
    assert.equal(batch.results[0].game_info.fields['Reply author'], undefined);
    assert.deepEqual(batch.results[0].game_info.image_urls, ['https://example.com/cover.png', 'https://example.com/screenshot.jpg']);
    assert.equal(batch.results[1].magnet, null);
    assert.equal(batch.results[1].magnet_status, 'not_found');
    assert.equal(batch.results[2].magnet_status, 'verification_required');
    const original = { complete: true, topics: ['100', '101', '102', '103'].map(id => ({ id, title: `Game ${id}` })) };
    const enriched = applyMagnets(original, [batch]);
    assert.equal(original.topics[0].magnet, undefined);
    assert.equal(enriched.complete, true); // Listing coverage is independent of magnet coverage.
    assert.equal(enriched.topics[0].title, 'Game 100');
    assert.equal(enriched.topics[3].magnet_status, 'not_checked');
    assert.equal(enriched.topics[3].game_info_status, 'not_checked');
    assert.equal(enriched.game_info_collection.available, 2);
    assert.equal(enriched.game_info_collection.complete, false);
    assert.equal(enriched.magnet_collection.available, 1);
    assert.equal(enriched.magnet_collection.remaining, 2);
    assert.equal(enriched.magnet_collection.complete, false);
    const invalid = await page.evaluate(collectMagnetBatch, { topicIds: ['104'], delayMs: 0 });
    assert.equal(invalid.results[0].magnet_status, 'invalid_magnet');
    assert.equal(invalid.results[0].magnet, null);
    const complete = await page.evaluate(collectMagnetBatch, { topicIds: ['100', '101'], delayMs: 0 });
    assert.equal(applyMagnets(original, [batch, { results: [{ id: '102', magnet: null, magnet_status: 'not_found' }, { id: '103', magnet: null, magnet_status: 'not_found' }] }]).magnet_collection.complete, true);
    assert.equal(complete.complete, true);
  } finally { await browser.close(); }
});
test('the public API fills magnets and peers without topic pages', async () => {
  const { applyApi } = await import('./api.mjs');
  const api = {
    format: { topic_id: ['tor_status', 'seeders', 'reg_time', 'tor_size_bytes', 'keeping_priority', 'keepers', 'seeder_last_seen', 'info_hash', 'topic_poster', 'leechers'] },
    update_time: 1791382052,
    result: {
      1: [2, 5, 1202587218, 628166656, 1, [], 0, '34892e6009191aa140adaf440ec88f0cbfb5d51d', 9, 2],
      2: [2, 7, 1202587218, 10, 1, [], 0, 'ABCDEF', 9, 0],
      3: [2, 1, 1202587218, 10, 1, [], 0, '6DFEE84456585BB461935A46FC0509D6EE35C7A9', 9, 0],
      99: [2, 1, 1202587218, 10, 1, [], 0, '6DFEE84456585BB461935A46FC0509D6EE35C7A9', 9, 0],
    },
  };
  const catalog = { topics: [
    { id: '1', title: 'A', seeders: 0 },
    { id: '2', title: 'B' },
    { id: '3', title: 'C', magnet: 'magnet:?xt=urn:btih:FROMPAGE', magnet_status: 'available' },
    { id: '4', title: 'D', magnet_status: 'not_checked' },
  ] };
  const { catalog: out, matched, unlisted } = applyApi(catalog, api);
  assert.deepEqual([matched, unlisted], [3, 1]);
  assert.equal(out.topics[0].magnet, 'magnet:?xt=urn:btih:34892E6009191AA140ADAF440EC88F0CBFB5D51D&tr=http%3A%2F%2Fbt3.t-ru.org%2Fann%3Fmagnet');
  assert.deepEqual([out.topics[0].seeders, out.topics[0].leechers, out.topics[0].size_bytes], [5, 2, 628166656]);
  assert.equal(out.topics[1].magnet, null, 'a malformed hash gives no magnet');
  assert.equal(out.topics[2].magnet, 'magnet:?xt=urn:btih:FROMPAGE', "the page's own magnet wins");
  assert.equal(out.topics[3].magnet_status, 'not_checked', 'topics missing from the API are untouched');
});

test('the daily refresh adds feed releases, follows re-uploads and drops deleted topics', async () => {
  const { parseFeed, refreshCatalog } = await import('./refresh.mjs');
  const feed = parseFeed(`<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
    <entry><id>tag:rto.feed,2026-10-07:/t/7</id><link href="https://rutracker.org/forum/viewtopic.php?t=7"/>
      <updated>2026-10-07T10:00:00+03:00</updated><title type="html"><![CDATA[[PS5] New Game [EUR / RUS / ENG / v1.000 / Лицензия] [6.17 GB]]]></title>
      <author><name>uploader</name></author></entry>
    <entry><link href="https://rutracker.org/forum/viewtopic.php?t=1"/><title>[Обновлено] [PS5] Old Game &amp; More [v1.002]</title></entry>
    <entry><link href="https://rutracker.org/forum/viewtopic.php?t=8"/><title>Rules of the forum</title></entry>
    <entry><link href="https://rutracker.org/forum/viewtopic.php?t=9"/><title>[PS5] Not registered yet</title></entry>
  </feed>`);
  assert.deepEqual(feed.map(e => [e.id, e.title]), [['7', '[PS5] New Game [EUR / RUS / ENG / v1.000 / Лицензия] [6.17 GB]'],
    ['1', '[PS5] Old Game & More [v1.002]'], ['8', 'Rules of the forum'], ['9', '[PS5] Not registered yet']]);
  const row = hash => [2, 3, 1791382052, 6624000000, 1, [], 0, hash, 1, 1];
  const api = { format: { topic_id: ['tor_status', 'seeders', 'reg_time', 'tor_size_bytes', 'keeping_priority', 'keepers', 'seeder_last_seen', 'info_hash', 'topic_poster', 'leechers'] },
    update_time: 1791382052, result: Object.fromEntries([1, 2, 3, 4, 5, 6, 7, 8].map(id => [id, row(String(id).repeat(40))])) };
  const topics = [1, 2, 3, 4, 5, 6, 10].map(id => ({ id: String(id), title: `[PS5] Game ${id}`, size: '1.00 GB',
    magnet: `magnet:?xt=urn:btih:${String(id).repeat(40)}`, magnet_status: 'available', game_info: { genre: 'Action' } }));
  topics[1].magnet = `magnet:?xt=urn:btih:${'F'.repeat(40)}&tr=x`; // re-uploaded since
  const platform = { tag: 'PS5' };
  const { catalog, added, renamed, removed } = refreshCatalog({ topics, topic_count: 7 }, feed, api, platform);
  assert.deepEqual([added, renamed, removed], [1, 1, 1], 'topic 10 is gone from the API; 8 is not a PS5 release; 9 has no torrent');
  const byId = Object.fromEntries(catalog.topics.map(t => [t.id, t]));
  assert.equal(byId[1].title, '[PS5] Old Game & More [v1.002]');
  assert.equal(byId[1].game_info.genre, 'Action', 'details survive a rename');
  assert.match(byId[2].magnet, /^magnet:\?xt=urn:btih:2{40}&tr=/, 'a re-uploaded torrent gets its new magnet');
  assert.equal(byId[3].magnet, `magnet:?xt=urn:btih:${'3'.repeat(40)}`, 'an unchanged page magnet is kept');
  assert.deepEqual([byId[7].size, byId[7].magnet_status, byId[7].game_info, byId[7].seeders], ['6.17 GB', 'available', null, 3]);
  assert.equal(catalog.topic_count, catalog.topics.length);
  // Sizes: a re-upload of a different size gets a new label (the review's 1 GB → 8 GiB case);
  // a label that already matches the bytes keeps RuTracker's wording.
  const sized = refreshCatalog({ topics: [
    { id: '1', title: '[PS5] A', size: '1.00 GB', magnet: null },
    { id: '2', title: '[PS5] B', size: '6.2 GB', magnet: null },
  ] }, [], { ...api, result: { 1: row('1'.repeat(40)).map((v, i) => i === 3 ? 8 * 2 ** 30 : v),
    2: row('2'.repeat(40)).map((v, i) => i === 3 ? 6.2 * 2 ** 30 : v) } }, platform).catalog;
  assert.deepEqual(sized.topics.map(t => t.size), ['8.00 GB', '6.2 GB']);
  // A broken API answer (most topics missing) never empties the catalog.
  const broken = { ...api, result: { 1: api.result[1] } };
  assert.equal(refreshCatalog({ topics, topic_count: 7 }, [], broken, platform).catalog.topics.length, 7);
});
