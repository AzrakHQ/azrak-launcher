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
    <entry><link href="https://rutracker.org/forum/viewtopic.php?t=8"/><title>[PS4/PS5] Cross Game [EUR]</title></entry>
    <entry><link href="https://rutracker.org/forum/viewtopic.php?t=11"/><title>Rules of the forum</title></entry>
    <entry><link href="https://rutracker.org/forum/viewtopic.php?t=9"/><title>[PS5] Not registered yet</title></entry>
  </feed>`);
  assert.deepEqual(feed.map(e => [e.id, e.title]), [['7', '[PS5] New Game [EUR / RUS / ENG / v1.000 / Лицензия] [6.17 GB]'],
    ['1', '[PS5] Old Game & More [v1.002]'], ['8', '[PS4/PS5] Cross Game [EUR]'], ['11', 'Rules of the forum'], ['9', '[PS5] Not registered yet']]);
  const row = hash => [2, 3, 1791382052, 6624000000, 1, [], 0, hash, 1, 1];
  const api = { format: { topic_id: ['tor_status', 'seeders', 'reg_time', 'tor_size_bytes', 'keeping_priority', 'keepers', 'seeder_last_seen', 'info_hash', 'topic_poster', 'leechers'] },
    update_time: 1791382052, result: Object.fromEntries([1, 2, 3, 4, 5, 6, 7, 8].map(id => [id, row(String(id).repeat(40))])) };
  const topics = [1, 2, 3, 4, 5, 6, 10].map(id => ({ id: String(id), title: `[PS5] Game ${id}`, size: '1.00 GB',
    magnet: `magnet:?xt=urn:btih:${String(id).repeat(40)}`, magnet_status: 'available', game_info: { genre: 'Action' } }));
  topics[1].magnet = `magnet:?xt=urn:btih:${'F'.repeat(40)}&tr=x`; // re-uploaded since
  const platform = { tag: 'PS5' };
  const { catalog, added, renamed, removed } = refreshCatalog({ topics, topic_count: 7 }, feed, api, platform);
  assert.deepEqual([added, renamed, removed], [2, 1, 1], 'topic 10 is gone from the API; 8 has a torrent, whatever its tag; 9 and the rules (11) have none');
  assert.ok(catalog.topics.some(t => t.id === '8') && !catalog.topics.some(t => t.id === '11' || t.id === '9'));
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

test('PC releases are matched to their Steam app by name, never to soundtracks or other games', async () => {
  const { cleanName, normalize, bestMatch, steamBlock } = await import('./steam.mjs');
  assert.equal(cleanName('[GOG] Human: Fall Flat [x86] [ENG] [Unity3D]'), 'Human: Fall Flat');
  assert.equal(cleanName('Transport Fever 3: Deluxe Edition (2026) (v.40408) + 2 DLC [RUS + ENG + 11 / ENG + 1]'), 'Transport Fever 3: Deluxe Edition');
  assert.equal(cleanName('[Трилогия] Gothic / Готика [x86] [RUS] [Wine]'), 'Gothic');
  assert.equal(normalize('Age of Empires II: Definitive Edition™'), 'age empires 2');
  const items = [
    { type: 'app', id: 1092840, name: 'Celeste Soundtrack' },
    { type: 'app', id: 504230, name: 'Celeste' },
    { type: 'app', id: 3779560, name: 'Celeste Blue Wizard Luna' },
  ];
  assert.equal(bestMatch('Celeste', items).id, 504230);
  assert.equal(bestMatch('Age of Empires II Definitive Edition', [{ type: 'app', id: 813780, name: 'Age of Empires II: Definitive Edition' }]).id, 813780);
  assert.equal(bestMatch('Ori and the Blind Forest', [{ type: 'app', id: 1, name: 'Ori and the Will of the Wisps' }]), null);
  assert.equal(bestMatch('Some Indie Game', []), null);
  assert.equal(bestMatch("King's Bounty", [{ type: 'app', id: 2, name: "King's Bounty: Crossworlds" }]), null, 'a spin-off is another game');
  const cod = [{ type: 'app', id: 4435490, name: 'Call of Duty®: Modern Warfare® 4' }, { type: 'app', id: 7940, name: 'Call of Duty® 4: Modern Warfare® (2007)' }];
  assert.equal(bestMatch('Call of Duty 4: Modern Warfare', cod).id, 7940, 'the same words with the number elsewhere are another game');
  assert.equal(bestMatch('Call of Duty: Modern Warfare 4', cod).id, 4435490);
  assert.equal(bestMatch('Heroes of Might and Magic', [{ type: 'app', id: 3, name: 'Heroes Of Might And Magic III' }]), null);
  assert.equal(bestMatch('Metro 2033 Redux', [{ type: 'app', id: 286690, name: 'Metro 2033 Redux' }]).id, 286690);
  assert.equal(bestMatch('Slay the Spire 2 Slay the Spire 2', [{ type: 'app', id: 4, name: 'Slay the Spire 2' }]).id, 4, 'a number said twice is still one');
  const { collection } = await import('./steam.mjs');
  assert.equal(collection("[Антология] Assassin's Creed (2008-2014) [amd64] [RUS]"), true);
  assert.equal(collection('[Trilogy] Mass Effect (1 + 2 + 3) [amd64]'), true);
  assert.equal(collection('Bioshock Remastered + Bioshock 2 Remastered [x86]'), true);
  assert.equal(collection('Transport Fever 3 (v.40408) + 2 DLC [RUS]'), false, 'DLC is part of the game');
  const block = steamBlock(504230, {
    name: 'Celeste', short_description: '  Help Madeline  survive. ', genres: [{ description: 'Action' }],
    release_date: { date: 'Jan 25, 2018' }, platforms: { linux: true }, metacritic: { score: 88 },
    screenshots: [{ path_full: 'https://x/ss.jpg' }], movies: [{ highlight: true, hls_h264: 'https://x/m.m3u8' }],
  }, { portrait: 'https://x/p.jpg', hero: null, logo: null }, 'platinum', new Date('2026-10-14T00:00:00Z'));
  assert.deepEqual([block.short, block.linux, block.video, block.protondb, block.metacritic], ['Help Madeline survive.', true, 'https://x/m.m3u8', 'platinum', 88]);
});

test('console releases without a title ID get Steam art, by exact name only', async () => {
  const { bestMatch, needsSteamArt, cleanName } = await import('./steam.mjs');
  // Only releases the launcher can't find PlayStation art for.
  assert.equal(needsSteamArt({ title: '[PS5] Mewgenics [USA / RUS / v01.121.220 / Retail / ffpfsc] [4.37 GB]', game_info: null }), true);
  assert.equal(needsSteamArt({ title: '[PS5] Stray [EUR] [PPSA02100]', game_info: null }), false);
  assert.equal(needsSteamArt({ title: '[PS4] Stray [EUR]', game_info: { title_id: 'CUSA24899' } }), false);
  assert.equal(cleanName("[PS5] Darwin's Paradox! [Region Free / ENG / v01.000.003 / Retail / folder]"), "Darwin's Paradox!");
  const items = [
    { type: 'app', id: 1, name: "Darwin's Paradox" },          // an older, different game with a near name
    { type: 'app', id: 2, name: 'Ghost of Tsushima DIRECTOR\'S CUT' },
    { type: 'app', id: 3, name: 'Superliminal Soundtrack' },
  ];
  // Exact: punctuation and edition words aside, the same name; near names don't count.
  assert.equal(bestMatch('Ghost of Tsushima', items, { exact: true })?.id, 2);
  assert.equal(bestMatch('Superliminal', items, { exact: true }), null, 'a soundtrack is not the game');
  assert.equal(bestMatch("Darwin's Paradox! 2", items, { exact: true }), null, 'a near name is not the game');
  // Edition suffixes Steam's search doesn't find: the base game's name.
  const { baseName } = await import('./steam.mjs');
  assert.equal(baseName('SILENT HILL: Townfall - Deluxe Edition'), 'SILENT HILL: Townfall');
  assert.equal(baseName('007 First Light: Deluxe Edition'), '007 First Light');
  assert.equal(baseName('Atomic Heart (Ultimate Edition)'), 'Atomic Heart');
  assert.equal(baseName("Assassin's Creed™ Black Flag Resynced: Deluxe Edition"), "Assassin's Creed™ Black Flag Resynced");
  assert.equal(baseName('Mortal Kombat 1: Definitive Edition'), 'Mortal Kombat 1');
  assert.equal(baseName('Hades II'), 'Hades II', 'no suffix, no change');
  assert.equal(baseName('The Last of Us Part II'), 'The Last of Us Part II');
  // PC matching stays as lenient as before (four of five words); console matching doesn't.
  const near = [{ type: 'app', id: 4, name: 'Super Meat Boy Forever' }];
  assert.equal(bestMatch('Super Meat Boy Forever Plus', near)?.id, 4);
  assert.equal(bestMatch('Super Meat Boy Forever Plus', near, { exact: true }), null);
});

test('Steam matches whose artwork or rating could not be checked are looked up again', async () => {
  const { lookup, due } = await import('./steam.mjs');
  const now = new Date('2026-10-14T00:00:00Z');
  const realFetch = globalThis.fetch;
  // Steam's store answers; its artwork server is unreachable and ProtonDB overloaded. (Not
  // unreachable: that would skip ProtonDB for the rest of the run, and of these tests.)
  globalThis.fetch = async (url) => {
    const u = String(url);
    if (u.includes('storesearch')) return Response.json({ items: [{ type: 'app', id: 504230, name: 'Celeste' }] });
    if (u.includes('appdetails')) return Response.json({ 504230: { success: true, data: { name: 'Celeste' } } });
    if (u.includes('protondb')) return new Response('', { status: 503 });
    throw new TypeError('fetch failed');
  };
  let block;
  try { block = await lookup('Celeste [GOG]', now); } finally { globalThis.fetch = realFetch; }
  assert.equal(block.appid, 504230);
  assert.equal(block.partial, true);
  assert.equal(due({ steam: block }, now), true, 'retried on the next run');
  const complete = { appid: 1, assets: { portrait: 'p', hero: null, logo: 'l' }, protondb: 'gold', checked_at: now.toISOString() };
  assert.equal(due({ steam: complete }, now), false);
  assert.equal(due({ steam: complete }, now, true), true, '--retry re-checks missing artwork');
  const { protondb, ...older } = complete; // no ProtonDB answer, from before `partial`
  assert.equal(due({ steam: older }, now), true);
  assert.equal(due({ title: 'Celeste', steam: { appid: null, query: 'Celeste', checked_at: now.toISOString() } }, now), false);
  const cod = { ...complete, name: 'Call of Duty®: Modern Warfare® 4' };
  assert.equal(due({ title: 'Call of Duty 4: Modern Warfare [x86] [RUS] [Wine]', steam: cod }, now), true, 'an earlier match to another numbered game');
  assert.equal(due({ title: 'Call of Duty: Modern Warfare 4 (2026) [RUS + ENG]', steam: cod }, now), false);
  assert.equal(due({ title: 'Metro: Last Light Redux Next Gen [amd64]', steam: { ...complete, name: 'Metro: Last Light Redux' } }, now), false, 'a looser earlier match stays');
});

test('a match that only lacks its ProtonDB rating just asks ProtonDB', async () => {
  const { update } = await import('./steam.mjs');
  const asked = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    asked.push(String(url));
    return Response.json({ tier: 'gold' });
  };
  const steam = { appid: 504230, name: 'Celeste', assets: { portrait: 'p', hero: 'h', logo: 'l' }, checked_at: '2026-10-01T00:00:00Z' };
  let result;
  try { result = await update({ title: 'Celeste [GOG]', steam }); } finally { globalThis.fetch = realFetch; }
  assert.deepEqual(asked, ['https://www.protondb.com/api/v1/reports/summaries/504230.json']);
  assert.equal(result.protondb, 'gold');
  assert.equal(result.assets.portrait, 'p', 'the rest is kept');
});

test('releases never looked up come before lookups that keep failing', async () => {
  const { lookupOrder, due } = await import('./steam.mjs');
  const now = new Date('2026-10-14T00:00:00Z');
  // 1000 matches whose artwork check keeps failing, ahead of a new release in the catalog.
  const stuck = Array.from({ length: 1000 }, (_, i) => ({ id: i, steam: { appid: i, partial: true, protondb: null, checked_at: '2026-10-13T00:00:00Z' } }));
  const topics = [...stuck, { id: 'new' }];
  const todo = lookupOrder(topics.filter(t => due(t, now))).slice(0, 900);
  assert.equal(todo[0].id, 'new');
});

test('translations keep every number of the original; added ones are fine', async () => {
  const { keepsNumbers } = await import('./english.mjs');
  assert.equal(keepsNumbers('от третьего лица', '3rd person'), true);
  assert.equal(keepsNumbers('Версия 1.2.3 от 01.02.2020', 'Version 1.2.3 from 01.02.2020'), true);
  assert.equal(keepsNumbers('Версия 1,5', 'Version 1.5'), false, 'reformatted');
  assert.equal(keepsNumbers('01.02.2020', '2020'), false, 'dropped');
  assert.equal(keepsNumbers('2 из 4', '4 of 2'), false, 'reordered');
});

test('release names: versions, builds and extras go; the JS and Rust cleaners agree', async () => {
  const { cleanName } = await import('./steam.mjs');
  const cases = JSON.parse(await (await import('node:fs/promises')).readFile(new URL('./clean-names.json', import.meta.url), 'utf8'));
  for (const [title, want] of cases) assert.equal(cleanName(title), want, title);
});

test('a miss searched under an older cleaned name is looked up again', async () => {
  const { due } = await import('./steam.mjs');
  const now = new Date('2026-10-15T00:00:00Z');
  const title = 'Mafia III: Definitive Edition (v.build 25654741 (08.10.2026)) [Wine] [MULTi10]';
  const checked_at = now.toISOString();
  assert.equal(due({ title, steam: { appid: null, checked_at } }, now), true, 'no query: from before');
  assert.equal(due({ title, steam: { appid: null, query: 'Mafia III: Definitive Edition (v.build 25654741', checked_at } }, now), true);
  assert.equal(due({ title, steam: { appid: null, query: 'Mafia III: Definitive Edition', checked_at } }, now), false);
});

test('a release without details borrows the game facts of another release of the same game', async () => {
  const { borrowDetails } = await import('./refresh.mjs');
  const donor = { id: '6910307', title: '[PS5] Little Nightmares Enhanced Complete Edition [EUR / RUS / ENG / v01.004.000 / Retail / FPKG]',
    url: 'https://rutracker.net/forum/viewtopic.php?t=6910307', game_info_status: 'available',
    game_info: { source_url: 'https://rutracker.net/forum/viewtopic.php?t=6910307', title_id: 'PPSA10737', genre: 'Horror, Puzzle',
      developer: 'Tarsier Studios', image_urls: ['https://example.org/cover.jpg'], version: 'v01.004.000', region: 'EUR', format: 'PKG (FPKG)',
      interface_languages: 'Русский, Английский', fields: { 'Регион': 'EUR' } } };
  const fresh = { id: '6921989', title: '[PS5] Little Nightmares Enhanced Complete Edition [RUS / EUR / 01.004 / Retail / exfat] [10.1 GB]',
    url: 'https://rutracker.net/forum/viewtopic.php?t=6921989', game_info: null, game_info_status: 'not_collected' };
  const other = { id: '1', title: '[PS5] Little Nightmares II [EUR]', url: 'u', game_info: null, game_info_status: 'not_collected' };
  assert.equal(borrowDetails([donor, fresh, other]), 1);
  assert.deepEqual(fresh.game_info, { source_url: 'https://rutracker.net/forum/viewtopic.php?t=6921989', borrowed_from: '6910307',
    title_id: 'PPSA10737', genre: 'Horror, Puzzle', developer: 'Tarsier Studios', image_urls: ['https://example.org/cover.jpg'] });
  assert.equal(fresh.game_info_status, 'borrowed', 'the collector still reads its own details');
  assert.equal(other.game_info, null, 'another game');
  assert.equal(borrowDetails([donor, fresh, other]), 0, 'only once');
});

test('releases the launcher cannot play are left out by their tag', async () => {
  const { UNPLAYABLE } = await import('./platforms.mjs');
  for (const title of ['[PS VR Only] GORN [USA]', '[PS4 VR] Dreams', '[PS VR+PS Move] Job Simulator', '[PS4 Exclusive VR Only] Blood & Truth',
    '[PS4 PS2 Classics] God of War [NTSC-U]', '[PS4 PS2 Classis] Star Wars', '[PSP-PS4] LocoRoco', '[PS4 PSX Classics] Ape Escape', '[PS4 homebrew] Store'])
    assert.ok(UNPLAYABLE.test(title), title);
  for (const title of ['[PS4] Firewatch [EUR]', '[PS4 Exclusive] God of War III (3) Remastered [EUR]', '[PS4 Exclusive PlayLink] That\'s You!',
    'My Friend Peppa Pig [EUR/RUS]', '[PS5] Ghost of Tsushima', '[PS4] Vampyr [EUR]', '[PS4] Moving Out [EUR]'])
    assert.ok(!UNPLAYABLE.test(title), title);
});

test("Steam artwork comes from Steam's own list, hashed paths included", async () => {
  const { lookup, fillAssets } = await import('./steam.mjs');
  const realFetch = globalThis.fetch;
  const listed = id => ({ id, success: 1, assets: { asset_url_format: `steam/apps/${id}/\${FILENAME}?t=1`,
    library_capsule: 'abc/library_capsule.jpg', library_capsule_2x: 'abc/library_capsule_2x.jpg', library_hero: 'def/library_hero.jpg' } });
  globalThis.fetch = async (url, options) => {
    const u = String(url);
    if (u.includes('storesearch')) return Response.json({ items: [{ type: 'app', id: 3486530, name: 'Stupid Never Dies' }] });
    if (u.includes('appdetails')) return Response.json({ 3486530: { success: true, data: { name: 'Stupid Never Dies' } } });
    if (u.includes('GetItems')) {
      const ids = JSON.parse(new URL(u).searchParams.get('input_json')).ids.map(i => i.appid);
      const dlc = id => ({ id, success: 1, assets: { asset_url_format: `steam/apps/${id}/\${FILENAME}`, hero_capsule_2x: 'h/hero_capsule_2x.jpg' } });
      return Response.json({ response: { store_items: ids.map(id => id === 7 ? { id, success: 2 } : id === 8 ? { id, success: 1, assets: {} }
        : id === 9 ? dlc(id) : listed(id)) } });
    }
    if (options?.method === 'HEAD') return new Response('', { status: 404 });
    return Response.json({ tier: 'gold' });
  };
  try {
    const block = await lookup('Stupid Never Dies [Wine]', new Date('2026-10-22T00:00:00Z'));
    assert.equal(block.assets.portrait, 'https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/3486530/abc/library_capsule_2x.jpg?t=1');
    assert.equal(block.assets.hero, 'https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/3486530/def/library_hero.jpg?t=1');
    assert.equal(block.assets.logo, null);
    assert.equal(block.partial, undefined, 'everything was answered');

    const topics = [
      { steam: { appid: 5, assets: { portrait: null, hero: null, logo: 'l' } } },
      { steam: { appid: 6, assets: { portrait: 'mine', hero: 'mine', logo: null } } }, // complete: not asked
      { steam: { appid: 7, assets: { portrait: null, hero: null, logo: null } } },     // Steam didn't answer
      { steam: { appid: 8, assets: { portrait: null, hero: null, logo: null } } },     // has none
      { steam: { appid: 9, assets: { portrait: null, hero: null, logo: null } } },     // DLC
      { steam: { appid: null } },
    ];
    assert.equal(await fillAssets(topics), 2);
    assert.match(topics[0].steam.assets.portrait, /\/5\/abc\/library_capsule_2x\.jpg/);
    assert.equal(topics[0].steam.assets.logo, 'l', 'the rest is kept');
    assert.equal(topics[1].steam.assets.portrait, 'mine');
    assert.equal(topics[2].steam.assets.portrait, null);
    assert.equal(topics[3].steam.assets.portrait, null);
    assert.equal(topics[4].steam.assets.portrait, 'https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/9/h/hero_capsule_2x.jpg');
  } finally { globalThis.fetch = realFetch; }
});
