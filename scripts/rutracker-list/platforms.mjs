// RuTracker forums the collector can read. In the console forums each release's title starts
// with the tag; the PC forums have no tag, so there every topic with a torrent is a release.
export const PLATFORMS = {
  ps5: { forumId: '546', tag: 'PS5', label: 'PS5' },
  ps4: { forumId: '973', tag: 'PS4', label: 'PS4' },
  // Games » Linux Games » Native Linux Games
  linux: { forumId: '1992', tag: null, label: 'native Linux' },
  // Games » Linux Games » Linux Games with Wine, DOSBox, and More
  wine: { forumId: '2059', tag: null, label: 'Linux (Wine, DOSBox…)' },
};

/** `--platform ps4|ps5` (default ps5) from command-line arguments. */
export function platformFrom(args) {
  const index = args.indexOf('--platform');
  const key = index < 0 ? 'ps5' : (args[index + 1] || '').toLowerCase();
  const platform = PLATFORMS[key];
  if (!platform) throw new Error(`--platform must be one of: ${Object.keys(PLATFORMS).join(', ')}`);
  return { key, ...platform, source: `https://rutracker.net/forum/viewforum.php?f=${platform.forumId}` };
}
