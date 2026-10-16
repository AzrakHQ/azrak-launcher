// RuTracker forums the collector can read. A release is a topic with a torrent; in the console
// forums most titles start with the console's tag ("[PS4] …", "[PS4 Exclusive] …").
export const PLATFORMS = {
  ps5: { forumId: '546', tag: 'PS5', label: 'PS5' },
  ps4: { forumId: '973', tag: 'PS4', label: 'PS4' },
  // Games » Linux Games » Native Linux Games
  linux: { forumId: '1992', tag: null, label: 'native Linux' },
  // Games » Linux Games » Linux Games with Wine, DOSBox, and More
  wine: { forumId: '2059', tag: null, label: 'Linux (Wine, DOSBox…)' },
};

/** Console releases the launcher can't play, by their title's tag: games that need a VR headset
 * ("[PS VR Only]", "[PS4 VR]", "[PS VR+PS Move]"), PS2/PS1/PSP/retro re-releases ("[PS4 PS2
 * Classics]", "[PSP-PS4]"; shadPS4's compatibility list has none of them), and homebrew apps.
 * Mirrored in listing.mjs, which runs in the browser on its own. */
export const UNPLAYABLE = /^\s*\[[^\]]*\b(?:VR|Move|homebrew|classics?|classis|PSP|PSX|PS2|PS1|SNES|Saturn)\b[^\]]*\]/i;

/** `--platform ps4|ps5` (default ps5) from command-line arguments. */
export function platformFrom(args) {
  const index = args.indexOf('--platform');
  const key = index < 0 ? 'ps5' : (args[index + 1] || '').toLowerCase();
  const platform = PLATFORMS[key];
  if (!platform) throw new Error(`--platform must be one of: ${Object.keys(PLATFORMS).join(', ')}`);
  return { key, ...platform, source: `https://rutracker.net/forum/viewforum.php?f=${platform.forumId}` };
}
