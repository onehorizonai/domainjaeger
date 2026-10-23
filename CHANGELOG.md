# Changelog

All notable changes to domainjaeger are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1] - 2026-10-23

### Changed

- The subjects in "Another subject…" and `cat list` are in A to Z order, not
  biggest first. The ones that are on still come first in the list.

### Fixed

- A topic of the English Wiktionary takes in the topics under it, in English,
  Latin and Spanish. Animals itself holds hardly an animal: they are under
  Mammals, Birds and Fish. A hunt in Animals found two English words of one
  part, and now finds hundreds. The words of each topic are downloaded again
  once.

## [0.3.0] - 2026-10-22

### Changed

- The categories on the home screen are a list where several can be on. Enter
  turns a category on or off, and esc goes back.
- "Another subject…" opens every subject `cat list` shows, to turn on and off
  the same way. Typing letters finds one. Before, you had to type its name.
- The home screen says "Only finance and tech" for two categories, not
  "finance+tech".
- Another language keeps the built-in categories of a mix and turns only the
  typed subjects off.

### Fixed

- An extension whose registry stops answering for a while no longer ends the
  hunt for good. The round used to say the dictionary had no more words, and
  any new setting, like another word before every word, found nothing after
  that. Now the round says the registry did not answer, the next round asks
  it again, and a new setting puts the extension back in the hunt.
- A hunt with the dot inside the word no longer switches to whole words, or
  crashes, when its registry stops answering.
- Changing the category no longer blinks the home screen, unless the
  dictionary has to be asked for its subjects.

## [0.2.0] - 2026-10-13

### Added

- A new kind of typo, the missing letter: numbers makes nmbe.rs, and
  nmbers.nl as a whole word. With all typos on, one word in ten gets such a
  name. Picked on its own, every word does.
- Typos are a choice: all of them, none, or one kind. Pick it on the home
  screen, type `typos missing`, or start with `--typos missing`. The kinds
  are doubled, dropped, sound, swapped and missing. `--no-typos` still works.

### Changed

- A category is found however it is typed. Capitals and extra spaces do not
  matter, and neither does a last s: `cat animal` finds Animals, and `cat Off`
  turns the category off.

## [0.1.0] - 2026-10-12

The first public version.

### Added

- A hunt for available domain names made of real words, with the extension
  as the end of the word: cook.ing, sn.app, tod.ai.
- Four languages: English, Latin and Spanish from the English Wiktionary,
  Dutch from WikiWoordenboek.
- Availability checked at the registry: DNS first, then RDAP, then WHOIS,
  with each registry lookup calibrated before it is trusted.
- Name styles: the dot inside the word, the whole word before the dot
  (lemon.nl), small typos of either, and a word of your own before or after
  every word (headspeed, speedbox).
- Categories built on dictionary topics (finance, tech, product), first names
  per language, and any dictionary topic as a category of its own.
- The meaning of every available word, in a few words.
- A warning on words the dictionary labels vulgar, offensive, sexual or
  violent, and a setting to leave them out.
- An interactive home screen with a Start line, a list of choices per
  setting and an Advanced page. It is drawn in place and falls back to a
  plain screen when the input is not a terminal.
- A command for every setting, and a flag for every setting.
- A cache that remembers every verdict for four weeks and the place in the
  dictionary between sessions.
- Export to Excel, with a sheet of available names and a sheet of everything
  that was checked.
- `hunt`, `check`, `export`, `refresh` and `stats` as commands that run
  without the home screen.

### Known limits

- Tested on macOS (Apple Silicon) only. Linux and Windows builds come out of
  CI, but nobody has verified them yet.
- The registries of `.es`, `.as`, `.al` and `.na` do not answer public
  lookups, so names there are never reported as available.

[Unreleased]: https://github.com/onehorizonai/domainjaeger/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/onehorizonai/domainjaeger/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/onehorizonai/domainjaeger/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/onehorizonai/domainjaeger/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/onehorizonai/domainjaeger/releases/tag/v0.1.0
