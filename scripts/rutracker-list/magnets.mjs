/** Runs in an ordinary browser page; reads HTML links without following magnets. */
export async function collectMagnetBatch({ topicIds, delayMs = 1500, concurrency = 1 }) {
  if (document.location.origin !== 'https://rutracker.net') throw new Error('Expected a RuTracker browser page');
  if (!Array.isArray(topicIds) || topicIds.some(id => !/^\d+$/.test(id))
      || !Number.isFinite(delayMs) || delayMs < 0
      || !Number.isSafeInteger(concurrency) || concurrency < 1 || concurrency > 4) throw new Error('Invalid magnet collection arguments');
  const results = [];
  const ids = [...new Set(topicIds)];
  function extractInfo(doc, base) {
    const body = doc.querySelector('.post_body');
    if (!body) return null;
    const clean = text => (text || '').replace(/\s+/g, ' ').trim();
    const aliases = {
      'год выпуска': 'release_year', 'год выхода': 'release_year', 'дата выхода': 'release_date',
      'release date': 'release_date', 'release year': 'release_year',
      'жанр': 'genre', 'жанры': 'genre', 'genre': 'genre',
      'разработчик': 'developer', 'developer': 'developer',
      'издательство': 'publisher', 'издатель': 'publisher', 'publisher': 'publisher',
      'локализатор': 'localizer', 'код диска': 'title_id', 'код игры': 'title_id', 'title id': 'title_id',
      'регион игры': 'region', 'регион': 'region', 'region': 'region',
      'тип издания': 'edition', 'платформа': 'platform', 'platform': 'platform',
      'мультиплеер': 'multiplayer', 'multiplayer': 'multiplayer',
      'версия игры': 'version', 'версия': 'version', 'version': 'version',
      'формат образа': 'format', 'формат': 'format', 'format': 'format',
      'минимальная версия прошивки': 'minimum_firmware', 'прошивка': 'minimum_firmware',
      'firmware': 'minimum_firmware', 'minimum firmware': 'minimum_firmware',
      'работоспособность проверена релизером': 'release_test_notes',
      'язык интерфейса': 'interface_languages', 'interface language': 'interface_languages',
      'язык озвучки': 'audio_languages', 'audio language': 'audio_languages',
      'язык субтитров': 'subtitle_languages', 'возраст': 'age_rating', 'age rating': 'age_rating',
    };
    const copy = body.cloneNode(true);
    copy.querySelectorAll('.sp-wrap, .quote-wrap, script, style').forEach(el => el.remove());
    copy.querySelectorAll('br').forEach(el => el.replaceWith(doc.createTextNode('\n')));
    copy.querySelectorAll('.post-b, b, strong').forEach(el => {
      if (/^\s*:/.test(el.nextSibling?.textContent || '')) el.before(doc.createTextNode('\n'));
    });
    const lines = copy.textContent.split('\n').map(clean).filter(Boolean);
    const details = {};
    const info = { source_url: base.href, fields: details };
    // Factual header labels only. Do not copy long promotional descriptions or replies.
    for (const line of lines) {
      if (/^(?:описание|description|synopsis|об игре)(?:\s|:|$)/i.test(line)) break;
      const match = /^([^:]{2,90}):\s*(.+)$/.exec(line);
      if (!match) continue;
      const label = clean(match[1]);
      if (/^(?:описание|description|об игре)$/i.test(label)) break;
      const value = clean(match[2]).slice(0, 1500);
      if (Object.keys(details).length >= 60) break;
      details[label] = value;
      const key = aliases[label.toLowerCase()];
      if (key) info[key] = value;
    }
    const heading = lines[0];
    info.name = heading && !heading.includes(':') && heading.length < 180
      ? heading : clean(doc.querySelector('#topic-title')?.textContent).replace(/^\[PS\d\]\s*/i, '').split(' [')[0];
    if (!info.title_id) info.title_id = /\b(?:PPSA|CUSA)\d{5}\b/.exec(JSON.stringify(details))?.[0] || null;
    const urls = new Set();
    for (const img of body.querySelectorAll('img.postImg, var.postImg')) {
      const raw = img.getAttribute('src') || img.getAttribute('title');
      if (!raw) continue;
      try { const url = new URL(raw, base); if (['http:', 'https:'].includes(url.protocol)) urls.add(url.href); } catch { /* Ignore invalid image URLs. */ }
      if (urls.size >= 80) break;
    }
    info.image_urls = [...urls];
    const update = lines.find(line => /(?:торрент|раздача)\s+обновл[её]н|(?:torrent|release)\s+updated/i.test(line));
    info.release_update = update?.slice(0, 500) || null;
    const summary = [info.name, info.genre, info.developer && `Developer: ${info.developer}`,
      info.version && `Version: ${info.version}`, info.minimum_firmware && `Minimum firmware: ${info.minimum_firmware}`];
    info.release_summary = summary.filter(Boolean).join(' · ');
    return info;
  }
  const ok = result => ['available', 'not_found'].includes(result.magnet_status);
  // A few topics at a time (at most 4), in order; stop at the first failure.
  for (let start = 0; start < ids.length; start += concurrency) {
    const group = await Promise.all(ids.slice(start, start + concurrency).map(readTopic));
    const failed = group.findIndex(result => !ok(result));
    results.push(...(failed < 0 ? group : group.slice(0, failed + 1)));
    if (failed >= 0) break;
    if (results.length < ids.length && delayMs) await new Promise(done => setTimeout(done, delayMs));
  }
  return { results, complete: results.length === ids.length && results.every(ok),
    collected_at: new Date().toISOString() };

  async function readTopic(id) {
    let result = { id, magnet: null, magnet_status: 'not_found', game_info: null, game_info_status: 'not_found' };
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 45_000);
    try {
      const response = await fetch(`/forum/viewtopic.php?t=${id}`, {
        credentials: 'same-origin', signal: controller.signal,
      });
      const charset = /charset\s*=\s*["']?([^\s;"']+)/i.exec(response.headers.get('content-type') || '')?.[1] || 'windows-1251';
      const html = new TextDecoder(charset).decode(await response.arrayBuffer());
      const doc = new DOMParser().parseFromString(html, 'text/html');
      const url = new URL(response.url);
      if (doc.querySelector('#challenge-form, #cf-challenge-running')
          || /just a moment|performing security verification/i.test(doc.title)) {
        result.magnet_status = 'verification_required';
      } else if (doc.querySelector('form#login-form-full')) {
        result.magnet_status = 'login_required';
      } else if (!response.ok) {
        result.magnet_status = `http_${response.status}`;
      } else if (url.origin !== document.location.origin || url.pathname !== '/forum/viewtopic.php'
          || url.searchParams.get('t') !== id || !doc.querySelector('#topic-title')) {
        result.magnet_status = 'unrecognized_topic';
      } else {
        result.game_info = extractInfo(doc, url);
        result.game_info_status = result.game_info ? 'available' : 'not_found';
        // Only the official torrent panel link for this ID, never links in replies.
        const anchor = [...doc.querySelectorAll('a.magnet-link[href]')]
          .find(link => link.getAttribute('data-topic_id') === id);
        if (anchor) {
          const magnet = new URL(anchor.getAttribute('href'));
          const valid = magnet.protocol === 'magnet:' && magnet.searchParams.getAll('xt')
            .some(xt => /^urn:btih:(?:[a-f\d]{40}|[a-z2-7]{32})$/i.test(xt));
          result.magnet_status = valid ? 'available' : 'invalid_magnet';
          if (valid) result.magnet = magnet.href;
        }
      }
    } catch (error) {
      result = { ...result, magnet_status: 'request_error', error: error.message };
    } finally {
      clearTimeout(timeout);
    }
    if (!ok(result)) result.game_info_status = result.magnet_status;
    return result;
  }
}

/** Normalize observed header variants and exclude prose mistaken for labels. */
export function normalizeGameInfo(info) {
  if (!info) return null;
  const rules = [
    [/^(?:год выпуска|год выхода|release year)$/i, 'release_year'],
    [/^(?:дата выпуска|дата выхода(?: игры)?|release date)$/i, 'release_date'],
    [/^(?:жанр|жанры|genre)$/i, 'genre'],
    [/^(?:разработчик|developer)$/i, 'developer'],
    [/^(?:издательство|издатель|publisher)$/i, 'publisher'],
    [/^локализатор$/i, 'localizer'],
    [/^(?:код диска|код игры|title id)$/i, 'title_id'],
    [/^(?:регион(?: игры| игр)?|region)$/i, 'region'],
    [/^тип издания$/i, 'edition'],
    [/^(?:платформа|platform)$/i, 'platform'],
    [/^(?:мультиплеер|multiplayer)$/i, 'multiplayer'],
    [/^мультиплеер\s*\(локальный\)$/i, 'local_multiplayer'],
    [/^мультиплеер\s*\(интернет\)$/i, 'online_multiplayer'],
    [/^(?:версия(?: игры)?|version)$/i, 'version'],
    [/^(?:формат(?: образа| игры| раздачи)?|format)$/i, 'format'],
    [/^(?:минимальная верси[яй] прошивки(?: & софт)?|версия прошивки|прошивка|firmware|minimum firmware)$/i, 'minimum_firmware'],
    [/^работоспособность проверена релизером$/i, 'release_test_notes'],
    [/^(?:язык интерфейса(?: игры|\s*\(.+\))?|interface language)$/i, 'interface_languages'],
    [/^(?:язык озвучки(?:\s*\(.+\))?|audio language)$/i, 'audio_languages'],
    [/^язык субтитров$/i, 'subtitle_languages'],
    [/^(?:возраст|возрастное ограничение|age rating)$/i, 'age_rating'],
    [/^название$/i, 'name'],
    [/^вид раздачи$/i, 'release_type'],
    [/^(?:перевод|перевод \/ локализация)$/i, 'translation'],
    [/^размер (?:игры|раздачи)(?: после распаковки)?$/i, 'unpacked_size'],
    [/^ps5 pro enhanced$/i, 'ps5_pro_enhanced'],
    [/^(?:поддержка )?pssr 2\.0$/i, 'pssr_support'],
    [/^ps vr\s*\(2\)$/i, 'psvr2'],
    [/^спецификация тестового стенда$/i, 'test_environment'],
    [/^таблетка \/ взлом$/i, 'release_protection_label'],
    [/^особенность dlc$/i, 'dlc_notes'],
  ];
  const normalized = { ...info, fields: {} };
  for (const [label, value] of Object.entries(info.fields || {})) {
    const key = label.replace(/^технические данные\s*/i, '').replace(/\s+/g, ' ').trim();
    const rule = rules.find(([pattern]) => pattern.test(key));
    if (!rule) continue;
    normalized.fields[key] = value;
    normalized[rule[1]] = value;
  }
  if (!normalized.release_year && normalized.release_date) {
    normalized.release_year = /\b(?:19|20)\d{2}\b/.exec(normalized.release_date)?.[0] || null;
  }
  normalized.release_summary = [normalized.name, normalized.genre,
    normalized.developer && `Developer: ${normalized.developer}`,
    normalized.version && `Version: ${normalized.version}`,
    normalized.minimum_firmware && `Minimum firmware: ${normalized.minimum_firmware}`].filter(Boolean).join(' · ');
  return normalized;
}

/** Preserve listing metadata and record magnet coverage separately. */
export function applyMagnets(report, batches) {
  const results = new Map(batches.flatMap(batch => batch.results).map(result => [result.id, result]));
  const topics = report.topics.map(topic => {
    const result = results.get(topic.id);
    return { ...topic, magnet: result?.magnet ?? topic.magnet ?? null,
      magnet_status: result?.magnet_status ?? topic.magnet_status ?? 'not_checked',
      game_info: normalizeGameInfo(result?.game_info ?? topic.game_info ?? null),
      game_info_status: result?.game_info_status ?? topic.game_info_status ?? 'not_checked' };
  });
  const available = topics.filter(topic => topic.magnet_status === 'available').length;
  const missing = topics.filter(topic => topic.magnet_status === 'not_found').length;
  const infoAvailable = topics.filter(topic => topic.game_info_status === 'available').length;
  const infoMissing = topics.filter(topic => topic.game_info_status === 'not_found').length;
  return { ...report, topics, game_info_collection: {
    collected_at: batches.at(-1)?.collected_at ?? null,
    complete: infoAvailable + infoMissing === topics.length,
    total: topics.length, available: infoAvailable, not_found: infoMissing,
    remaining: topics.length - infoAvailable - infoMissing,
  }, magnet_collection: {
    collected_at: batches.at(-1)?.collected_at ?? null,
    complete: available + missing === topics.length,
    total: topics.length, available, not_found: missing,
    remaining: topics.length - available - missing,
    failures: [...results.values()].filter(result => !['available', 'not_found'].includes(result.magnet_status)),
  } };
}