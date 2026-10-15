const russian = /[\u0400-\u04ff]/;

export const fieldLabels = {
  'Год выпуска': 'Release year', 'Жанр': 'Genre', 'Разработчик': 'Developer',
  'Издательство': 'Publisher', 'Издатель': 'Publisher', 'Локализатор': 'Localization provider',
  'Мультиплеер': 'Multiplayer', 'Возрастное ограничение': 'Age rating', 'Возраст': 'Age rating',
  'Платформа': 'Platform', 'Код диска': 'Title ID', 'Код игры': 'Title ID',
  'Регион игры': 'Region', 'Регион игр': 'Region', 'Регион': 'Region',
  'Тип издания': 'Edition', 'Версия игры': 'Game version', 'Формат образа': 'Image format',
  'Формат игры': 'Game format', 'формат игры': 'Game format', 'Формат': 'Format',
  'Минимальная версия прошивки': 'Minimum firmware', 'Минимальная версий прошивки': 'Minimum firmware',
  'Работоспособность проверена релизером': 'Tested by the uploader',
  'Язык интерфейса': 'Interface languages', 'Язык интерфейса игры': 'Interface languages',
  'Язык озвучки': 'Audio languages', 'Дата выпуска': 'Release date', 'Вид раздачи': 'Release type',
  'Перевод': 'Translation', 'Название': 'Name', 'Перевод / Локализация': 'Translation / localization',
  'Поддержка PSSR 2.0': 'PSSR 2.0 support', 'Таблетка / Взлом': 'Crack / protection bypass',
  'Мультиплеер (интернет)': 'Online multiplayer', 'Мультиплеер(интернет)': 'Online multiplayer',
  'Мультиплеер (локальный)': 'Local multiplayer', 'Мультиплеер(локальный)': 'Local multiplayer',
  'Минимальная версия прошивки & Софт': 'Minimum firmware & software',
  'Спецификация тестового стенда': 'Test setup', 'Язык интерфейса (Субтитры)': 'Interface languages (subtitles)',
  'Формат раздачи': 'Release format', 'Версия прошивки': 'Firmware version',
  'Размер раздачи после распаковки': 'Unpacked release size', 'Особенность DLC': 'DLC notes',
  'Размер игры после распаковки': 'Installed game size', 'Размер игры': 'Game size',
  'Язык интерфейса (Screen Languages)': 'Interface languages (screen text)',
  'Язык озвучки (Voice)': 'Audio languages (voices)', 'Язык озвучки (комментаторы)': 'Commentary languages',
  'Язык озвучки (дополнение "Valhalla")': 'Audio languages (Valhalla DLC)',
  'Язык озвучки (дополнение Valhalla)': 'Audio languages (Valhalla DLC)',
  'Язык озвучки (основная игра)': 'Audio languages (base game)',
  'Язык интерфейса(EU)': 'Interface languages (EU)', 'Язык интерфейса(US)': 'Interface languages (US)',
  'Язык озвучки(EU)': 'Audio languages (EU)', 'Язык озвучки(US)': 'Audio languages (US)',
};

const glossary = {
  'Китайский (традиционное письмо)': 'Chinese (Traditional)',
  'Китайский (упрощенное письмо)': 'Chinese (Simplified)',
  'Китайский (упрощённое письмо)': 'Chinese (Simplified)',
  'Португальский (Португалия)': 'Portuguese (Portugal)', 'Португальский (Бразилия)': 'Portuguese (Brazil)',
  'Французский (Франция)': 'French (France)', 'Французский (Канада)': 'French (Canada)',
  'Испанский (Испания)': 'Spanish (Spain)', 'Испанский (Латинская Америка)': 'Spanish (Latin America)',
  'Текст и Звук': 'Text and audio', 'Полная локализация': 'Fully localized',
  'Цифровой дамп': 'Digital dump', 'Минимальная версия прошивки': 'Minimum firmware',
  'Работает на версиях ПО от': 'Runs on firmware versions from',
  'работает только до': 'works only up to', 'внешний': 'external', 'внутренний': 'internal',
  'Русский': 'Russian', 'Английский': 'English', 'Немецкий': 'German', 'Французский': 'French',
  'Итальянский': 'Italian', 'Испанский': 'Spanish', 'Японский': 'Japanese', 'Корейский': 'Korean',
  'Китайский': 'Chinese', 'Португальский': 'Portuguese', 'Польский': 'Polish', 'Турецкий': 'Turkish',
  'Арабский': 'Arabic', 'Украинский': 'Ukrainian', 'Чешский': 'Czech', 'Венгерский': 'Hungarian',
  'Шведский': 'Swedish', 'Нидерландский': 'Dutch', 'Датский': 'Danish', 'Норвежский': 'Norwegian',
  'Финский': 'Finnish', 'Греческий': 'Greek', 'Румынский': 'Romanian', 'Тайский': 'Thai',
  'Индонезийский': 'Indonesian', 'Вьетнамский': 'Vietnamese', 'Хинди': 'Hindi',
  'Лицензия': 'Retail', 'Репак': 'Repack', 'Демо': 'Demo', 'Текст': 'Text', 'Звук': 'Audio',
  'Экшен': 'Action', 'Приключения': 'Adventure', 'Головоломка': 'Puzzle', 'Платформер': 'Platformer',
  'Стратегия': 'Strategy', 'Симулятор': 'Simulation', 'Аркада': 'Arcade', 'Гонки': 'Racing',
  'Шутер': 'Shooter', 'Спорт': 'Sports', 'Файтинг': 'Fighting', 'Ролевая игра': 'Role-playing',
  'Отсутствует': 'None', 'Нет': 'No', 'Да': 'Yes', 'Требуется': 'Required',
  'Европа': 'Europe', 'Америка': 'America', 'ПО': 'firmware', 'хост': 'host',
  'ГБ': 'GB', 'МБ': 'MB', 'ТБ': 'TB',
};
const escape = text => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const replacements = Object.entries(glossary).sort(([a], [b]) => b.length - a.length)
  .map(([source, target]) => [new RegExp(`(?<![\\p{L}])${escape(source)}(?![\\p{L}])`, source === 'ПО' ? 'gu' : 'giu'), target]);

export function hasRussian(text) { return typeof text === 'string' && russian.test(text); }
export function numericTokens(text) { return text.match(/\d+(?:[.,/](?:\d+[a-z]*\d*|x+|\?+))*/gi) || []; }
export function protectTechnical(text) {
  const tokens = [];
  const value = text.replace(/\b(?:[A-Za-z][A-Za-z0-9_.+-]*\d[A-Za-z0-9_.+-]*|v?\d+(?:[.,/][\dA-Za-z?]+)*(?:[+-])?)/g, token => {
    const marker = `ZXQ${String(tokens.length).padStart(5, '0')}QXZ`;
    tokens.push([marker, token]);
    return marker;
  });
  return { value, restore(translated) {
    for (const [marker, token] of tokens) {
      if (!translated.includes(marker)) throw new Error(`Technical placeholder lost: ${marker}`);
      translated = translated.replaceAll(marker, token);
    }
    return translated;
  } };
}
export function isProtected(key, value) {
  return ['id', 'title_id', 'author'].includes(key) || /url(?:s)?$/i.test(key)
    || key === 'magnet' || /^(?:https?:|magnet:)/i.test(value);
}
export function knownEnglish(text) {
  let value = text.replace(/(\d)(?:\.х+|х)(?!\p{L})/gu, match => match.replaceAll('х', 'x'));
  value = value.replace(/^(?:Т|T)\s*-\s*От 13 лет$/i, 'T (Teen, ages 13+)')
    .replace(/^(?:М|M)\s*-\s*От 17 лет$/i, 'M (Mature, ages 17+)')
    .replace(/^E\s*-\s*Для всех$/i, 'E (Everyone)');
  for (const [pattern, target] of replacements) value = value.replace(pattern, target);
  return value;
}

const LOOKALIKES = { А: 'A', В: 'B', Е: 'E', К: 'K', М: 'M', Н: 'H', О: 'O', Р: 'P', С: 'C', Т: 'T', Х: 'X', У: 'Y',
  а: 'a', е: 'e', о: 'o', р: 'p', с: 'c', у: 'y', х: 'x' };

/** In words that are otherwise Latin letters and digits, Cyrillic look-alikes become Latin. */
export function latinLookalikes(value) {
  if (typeof value !== 'string') return value;
  // Russian-layout typos between digits: "1ю00" is "1.00", "1б5" is "1,5".
  value = value.replace(/(\d)ю(?=\d)/g, '$1.').replace(/(\d)б(?=\d)/g, '$1,');
  return value.replace(/[\p{L}\d+]+/gu, word => {
    const cyrillic = [...word].filter(ch => /\p{Script=Cyrillic}/u.test(ch));
    if (!cyrillic.length || !/[A-Za-z\d]/.test(word) || !cyrillic.every(ch => LOOKALIKES[ch])) return word;
    return [...word].map(ch => LOOKALIKES[ch] || ch).join('');
  });
}

export function prepareCatalog(input) {
  const report = structuredClone(input);
  // Repair run-on extraction boundaries rather than translating appended advertising.
  const trim = value => typeof value === 'string'
    ? value.split(/Описание|Description(?:\s|:)/i)[0].trim() : value;
  for (const topic of report.topics) {
    const info = topic.game_info;
    if (!info) continue;
    for (const [key, value] of Object.entries(info.fields || {})) info.fields[key] = trim(value);
    for (const key of ['age_rating', 'format', 'minimum_firmware']) info[key] = trim(info[key]);
    for (const [key, value] of Object.entries(info.fields || {})) {
      if (!/формат/i.test(key) || typeof value !== 'string') continue;
      const boundary = /Минимальная верси[яй] прошивки(?: & Софт)?:\s*/i.exec(value);
      if (!boundary) continue;
      const firmware = value.slice(boundary.index + boundary[0].length).trim();
      info.fields[key] = value.slice(0, boundary.index).trim();
      info.format = info.fields[key];
      if (!info.minimum_firmware) {
        info.minimum_firmware = firmware;
        info.fields['Минимальная версия прошивки'] = firmware;
      }
    }
    // Codes typed with Cyrillic look-alike letters ("СUSA12345", "Е10+") are the Latin code.
    for (const [key, value] of Object.entries(info.fields || {})) info.fields[key] = latinLookalikes(value);
    for (const [key, value] of Object.entries(info)) if (typeof value === 'string') info[key] = latinLookalikes(value);
    // Rebuilt from translated facts later; do not translate duplicated summaries.
    delete info.release_summary;
  }
  return report;
}

export function translationInputs(report) {
  const values = new Set();
  function walk(value, key = '') {
    if (typeof value === 'string') {
      if (!isProtected(key, value)) { const text = knownEnglish(value); if (hasRussian(text)) values.add(text); }
    } else if (Array.isArray(value)) value.forEach(item => walk(item, key));
    else if (value && typeof value === 'object') for (const [name, item] of Object.entries(value)) {
      if (hasRussian(name) && !fieldLabels[name]) values.add(knownEnglish(name));
      walk(item, name);
    }
  }
  walk(report);
  return [...values];
}

export function englishCatalog(report, translations) {
  function convert(value, key = '') {
    if (typeof value === 'string') {
      if (isProtected(key, value)) return value;
      const text = knownEnglish(value);
      const translated = hasRussian(text) ? translations[text] : text;
      if (typeof translated !== 'string' || hasRussian(translated)) throw new Error(`Untranslated text: ${text}`);
      return translated.replace(/\bfrom (\d+) years old\b/gi, 'ages $1+')
        .replace(/^Required system (.+?)(?: and higher)?$/i, 'Requires firmware $1')
        .replace(/\(from (\d{1,2}\.\d{1,2}\.\d{2,4})\)/g, '(dated $1)')
        .replace(/\.(?=(?:Interface|Audio|Translation|Minimum|Age)\b)/g, '. ')
        .replace(/translation into Russian from the Yandex neuron/gi, "Yandex's AI translation into Russian");
    }
    if (Array.isArray(value)) return value.map(item => convert(item, key));
    if (!value || typeof value !== 'object') return value;
    const result = {};
    for (const [name, item] of Object.entries(value)) {
      let label = fieldLabels[name] || (hasRussian(name) ? convert(name) : name);
      const translated = convert(item, name);
      if (Object.hasOwn(result, label)) {
        if (JSON.stringify(result[label]) === JSON.stringify(translated)) continue;
        let suffix = 2;
        while (Object.hasOwn(result, `${label} (${suffix})`)) suffix++;
        label = `${label} (${suffix})`;
      }
      result[label] = translated;
    }
    return result;
  }
  const result = convert(report);
  for (const topic of result.topics) {
    const info = topic.game_info;
    if (!info) continue;
    if (/^(?:No|None)$/i.test(info.localizer || '')) info.localizer = 'None';
    if (/^(?:No|None)$/i.test(info.translation || '')) info.translation = 'None';
    for (const key of ['Localization provider', 'Translation']) {
      if (/^(?:No|None)$/i.test(info.fields?.[key] || '')) info.fields[key] = 'None';
    }
    info.release_summary = [info.name, info.genre, info.developer && `Developer: ${info.developer}`,
      info.version && `Version: ${info.version}`, info.minimum_firmware && `Minimum firmware: ${info.minimum_firmware}`]
      .filter(Boolean).join(' · ');
  }
  return result;
}
const TRANSLIT = { а: 'a', б: 'b', в: 'v', г: 'g', д: 'd', е: 'e', ё: 'e', ж: 'zh', з: 'z', и: 'i', й: 'y', к: 'k', л: 'l', м: 'm',
  н: 'n', о: 'o', п: 'p', р: 'r', с: 's', т: 't', у: 'u', ф: 'f', х: 'kh', ц: 'ts', ч: 'ch', ш: 'sh', щ: 'shch', ъ: '', ы: 'y',
  ь: '', э: 'e', ю: 'yu', я: 'ya' };

/** Last resort for text the translator returned unchanged: Latin letters instead of Cyrillic. */
export function transliterate(value) {
  return value.replace(/\p{Script=Cyrillic}/gu, ch => {
    const latin = TRANSLIT[ch.toLowerCase()] ?? '';
    return ch === ch.toLowerCase() ? latin : latin.charAt(0).toUpperCase() + latin.slice(1);
  });
}

export const numericSignature = text => text.match(/\d+(?:[.,/]\d+|\.x+|\.\?+)*/gi) || [];
// Every number, version and date of the original is in the translation, unchanged and in order; a
// number the translation adds ("от третьего лица" → "3rd person") is fine.
export function keepsNumbers(text, translation) {
  const want = numericSignature(text);
  let found = 0;
  for (const token of numericSignature(translation)) if (token === want[found]) found++;
  return found === want.length;
}

