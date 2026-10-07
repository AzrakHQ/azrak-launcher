/** Self-contained DOM reader; runs unchanged in Playwright's page.evaluate(). */
export function readListing({ forumId = '546', tag = 'PS5' } = {}) {
  const base = new URL(document.location.href);
  const clean = value => (value || '').replace(/\s+/g, ' ').trim();
  const integer = element => {
    const value = clean(element?.textContent).replace(/[\s,\u00a0]/g, '');
    return /^\d+$/.test(value) ? Number(value) : null;
  };
  const challenge = document.querySelector('#challenge-form, #cf-challenge-running')
    || /just a moment|performing security verification/i.test(document.title);
  const login = document.querySelector('form#login-form-full');
  if (challenge || login) {
    return { status: challenge ? 'verification_required' : 'login_required', topics: [], pageUrls: [] };
  }
  if (base.hostname !== 'rutracker.net' || base.pathname !== '/forum/viewforum.php'
      || base.searchParams.get('f') !== forumId) {
    return { status: 'unexpected_page', topics: [], pageUrls: [] };
  }
  const anchors = [...document.querySelectorAll('a.torTopic[href], a.topictitle[href]')];
  if (!anchors.length) return { status: 'unrecognized_listing', topics: [], pageUrls: [] };
  const topics = new Map();
  let skipped = 0;
  for (const anchor of anchors) {
    const title = clean(anchor.textContent);
    const url = new URL(anchor.getAttribute('href'), base);
    const id = url.searchParams.get('t');
    const row = anchor.closest('tr');
    const released = tag ? title.toUpperCase().startsWith(`[${tag.toUpperCase()}]`)
      : Boolean(row?.querySelector('.vf-col-tor a.dl-stub')); // untagged forums: topics with a torrent
    if (url.origin !== base.origin || url.pathname !== '/forum/viewtopic.php'
        || !/^\d+$/.test(id || '') || !released) {
      skipped++;
      continue;
    }
    if (topics.has(id)) continue;
    topics.set(id, {
      id,
      title,
      url: `${base.origin}/forum/viewtopic.php?t=${id}`,
      size: clean(row?.querySelector('.vf-col-tor a.dl-stub')?.textContent) || null,
      seeders: integer(row?.querySelector('.seedmed b, b.seedmed')),
      leechers: integer(row?.querySelector('.leechmed b, b.leechmed')),
      author: clean(row?.querySelector('a.topicAuthor')?.textContent) || null,
    });
  }
  const offsets = new Set([0]);
  for (const anchor of document.querySelectorAll('a[href*="viewforum.php"]')) {
    const url = new URL(anchor.getAttribute('href'), base);
    if (url.origin !== base.origin || url.pathname !== base.pathname || url.searchParams.get('f') !== forumId) continue;
    const raw = url.searchParams.get('start') || '0';
    if (/^\d+$/.test(raw) && Number(raw) <= 1_000_000) offsets.add(Number(raw));
  }
  const ordered = [...offsets].sort((a, b) => a - b);
  // RuTracker's first-page navigation exposes the first offsets and final page.
  // Fill skipped middle links using the observed page size, not a fixed 50.
  const step = ordered[1];
  const last = ordered.at(-1);
  if (step && ordered.every(offset => offset % step === 0)) {
    for (let offset = 0; offset <= last; offset += step) offsets.add(offset);
  }
  const pageUrls = [...offsets].sort((a, b) => a - b).map(offset =>
    `${base.origin}/forum/viewforum.php?f=${forumId}${offset ? `&start=${offset}` : ''}`);
  return { status: 'ok', title: document.title, topics: [...topics.values()], pageUrls, skipped };
}

export function mergeTopics(pages) {
  const topics = new Map();
  for (const page of pages) for (const topic of page.topics) topics.set(topic.id, topic);
  return [...topics.values()].sort((a, b) => a.title.localeCompare(b.title));
}