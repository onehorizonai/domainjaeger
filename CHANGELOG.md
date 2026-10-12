# Changelog

All notable changes to domainjaeger are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - Unreleased

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

[0.1.0]: https://github.com/onehorizonai/domainjaeger/releases/tag/v0.1.0
