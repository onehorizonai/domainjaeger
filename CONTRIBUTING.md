# Contributing to domainjaeger

Thank you for wanting to make domainjaeger better. This page tells you how to
build it, how to test it without touching your own finds, and what a good pull
request looks like.

## Ways to help

- **Report a bug.** Use the bug form under
  [Issues](https://github.com/onehorizonai/domainjaeger/issues). Say what you typed,
  what you saw and what you expected.
- **Verify a platform.** domainjaeger is tested on macOS (Apple Silicon)
  only. A report of a full hunt on Linux or Windows, good or bad, is worth a
  lot.
- **Improve the words.** A new language, a better typo rule, a category that
  is missing, an extension whose registry can be asked after all.
- **Improve the writing.** The app and its documentation use plain words. If
  a sentence made you stop, it can be better.

For anything larger than a fix, open an issue first. It saves you from
building something that does not fit.

## Build and run

You need the current stable Rust toolchain ([rustup.rs](https://rustup.rs)).

```sh
git clone https://github.com/onehorizonai/domainjaeger
cd domainjaeger
cargo run --release                      # the app
cargo run --release -- --lang nl         # with flags
```

## Before you open a pull request

These three must pass. CI runs the same.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The tests run offline and take a few seconds.

## Test against a scratch cache, never your own

domainjaeger keeps your finds in `~/.domainjaeger/cache.sqlite`. While you
work on the code, point it somewhere else:

```sh
cargo run --release -- --db /tmp/scratch.sqlite
```

Do the same when the code under test touches the cache layout. A scratch
cache starts empty and anonymous, which is also the state every new user is
in.

Be gentle with the services behind it. Anonymous clients get 10 dictionary
requests a minute from Wikimedia, and registries limit lookups as well. A
round or two is a fine manual test. A loop is not.

## How the code is laid out

One module per concern, each with its tests at the bottom of the file:

- the **dictionary**: reading words, topics, meanings and first names from
  Wiktionary and WikiWoordenboek;
- the **names**: turning a word and an extension into candidates, typos
  included;
- the **check**: asking DNS, RDAP and WHOIS whether a name is free;
- the **hunt**: one round, from words to verdicts;
- the **cache**: what was checked, in SQLite;
- the **screen**: everything the terminal shows, and the menu;
- the **export**: the Excel file.

Where to find them in `src/`:

| file | holds |
| --- | --- |
| `main.rs` | the start: reads the command line and hands the app the real web, WHOIS and clock |
| `cli.rs` | the flags and subcommands |
| `app.rs` | the state of a session and the commands that change it |
| `app/setup.rs` | the lines of the setup, with their values and choices |
| `app/menu.rs` | the interactive home screen and the prompt |
| `app/rounds.rs` | a round, a word of your own, another look at old names |
| `app/finds.rs` | the list, the Excel export and the numbers |
| `hunt.rs` | one round, from dictionary pages to verdicts |
| `wiktionary.rs` | the two dictionaries |
| `hacks.rs` | names from a word and an extension, typos included |
| `check.rs` | DNS, RDAP and WHOIS, and whether a registry can be trusted |
| `tld.rs` | which extensions exist and which are open to everyone |
| `topics.rs`, `tone.rs`, `lang.rs` | categories, rude-word labels, languages |
| `cache.rs` | the SQLite file |
| `export.rs` | the Excel file |
| `ui.rs` | everything the terminal shows |
| `net.rs` | the doors to the outside world, and their stand-ins for tests |
| `testing.rs` | a small scripted world that tests hunt in |

Everything outside the program sits behind a small trait in `net.rs`: the web
requests (`Web`), the WHOIS lookups (`Whois`) and the clock (`Clock`). The
program gets the real ones when it starts. Tests hand in stand-ins that
answer from a script, which is why they need no network and no waiting. When
you add something that talks to the outside world, give it the same
treatment.

A test of a whole round reads like this:

```rust
let world = World::new();
world.registry("ing").taken(&["mak.ing"]);
world.finds("+ing$/", &["cooking", "making"]);
world.otherwise_quiet();
let mut hunter = world.hunter(&["ing"], testing::settings());
// ... run a round, then look at what it found and what it asked.
```

## Testing the menu

The home screen redraws itself in place, so reading its output as text tells
you little. It is tested by running the program in a pseudo-terminal, sending
it keys, and feeding what it prints to a terminal emulator. The emulator then
holds what a person would see after each key. A short Python script with `pty`
and [`pyte`](https://pypi.org/project/pyte/) is enough.

Check at 80 columns by 24 lines, the smallest window the menu is drawn in.
Also run the program with its input piped (`printf 'h\nq\n' | domainjaeger
--db /tmp/scratch.sqlite`) to see the screen it falls back to.

## Style

- **Plain words on screen.** "Extension", not TLD. "Parts", not syllables.
  Explain with an example rather than a term.
- **A calm screen.** Nothing starts on its own. No icons or emoji in the
  menu. One accent colour. Text lines up on one left edge.
- **Characters every font has.** Letters, punctuation, box drawing and block
  elements are safe. Many other symbols are missing from common terminal
  fonts or are drawn two cells wide.
- **Every setting works three ways:** a line in the setup with a list of
  choices, a command you can type, and a flag.
- **Comments say why.** The code says what.

## Commits and pull requests

- Keep a pull request to one subject. Two fixes are two pull requests.
- Write the commit subject as an instruction, in the present tense: "Skip
  extensions whose registry does not answer".
- Add or update tests for the behaviour you change.
- Update `README.md` when what a user sees or types changes, and add a line
  to `CHANGELOG.md` under Unreleased.
- Say in the description how you tested, and on which system.

Releases are described in [docs/RELEASING.md](docs/RELEASING.md).

## Conduct and licence

By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md). Your
contributions are licensed under the [MIT License](LICENSE), like the rest of
the project.

Found a security problem? Do not open an issue. See
[SECURITY.md](SECURITY.md).
