"""Shared set-up for the scripts that drive the launcher on hidden displays
(record_demo.py, ui_matrix.py, ui_audit.py).

They run the real launcher with a throwaway home folder. These helpers keep that home in step
with the launcher: which cached files to seed, which background downloads to keep off, and an
installed game to show, since an empty Library opens on "No games installed yet".
"""

import json
import shutil
from pathlib import Path

# The collector's browser profiles hold live sockets and aren't the launcher's.
CACHE_SKIP = ("logs", "browser-profile", "rutracker-list-browser")

# Nothing may download or update in the background during a run.
NO_UPDATES = {"kyty_auto_update": False, "app_auto_update": False, "shad_auto_update": False}

# A real title (artwork comes from the seeded cache when it has it).
STAND_IN_ID = "PPSA21564"  # ASTRO BOT: in-game on Linux, so Home shows a green result


def seed_cache(home: Path) -> None:
    """Copy your launcher cache (catalogs, artwork, compatibility) into the throwaway home."""
    real = Path.home() / ".cache/azrak"
    if real.exists():
        shutil.copytree(real, home / ".cache/azrak", ignore=shutil.ignore_patterns(*CACHE_SKIP), dirs_exist_ok=True)


def stand_in_game(games: Path, title_id: str = STAND_IN_ID) -> Path:
    """An inert installed game: real metadata and an eboot.bin that is not a program. Pair it
    with a stand-in emulator; nothing real is ever started."""
    game = games / f"{title_id}-app0"
    (game / "sce_sys").mkdir(parents=True, exist_ok=True)
    (game / "sce_sys/param.json").write_text(json.dumps(
        {"titleId": title_id, "localizedParameters": {"en-US": {"titleName": "ASTRO BOT"}}}))
    (game / "eboot.bin").write_bytes(b"stand-in for UI scripts; not a program\n")
    return game


def install_game(games: Path, game: Path | None) -> None:
    """Show `game` (a real folder, linked read-only) or else a stand-in as installed."""
    games.mkdir(parents=True, exist_ok=True)
    if game:
        (games / game.name).symlink_to(game.resolve())
    else:
        stand_in_game(games)
